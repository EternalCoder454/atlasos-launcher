#include "runners.h"

#include <KConfigGroup>
#include <KPluginMetaData>
#include <KRunner/AbstractRunner>
#include <KRunner/RunnerManager>
#include <KSharedConfig>

#include <QLoggingCategory>
#include <QMetaObject>
#include <QSet>
#include <QVariantList>
#include <QVariantMap>

#include <algorithm>

Q_LOGGING_CATEGORY(lcRunners, "atlas.launcher.runners", QtInfoMsg)

namespace
{
constexpr int kMaxMatches = 200;
constexpr int kMaxQueryLength = 256;

// The plugins the launcher draws itself, and the ones another part of the
// launcher replaces. Both the bare names krunnerrc's keys sometimes use and
// the `krunner_` ids of Plasma 6 are listed, so a rename upstream still keeps
// them out.
bool drawnByLauncher(const QString &id)
{
    static const QSet<QString> ids{
        // Drawn by the launcher.
        QStringLiteral("services"),
        QStringLiteral("krunner_services"),
        QStringLiteral("calculator"),
        QStringLiteral("krunner_calculator"),
        QStringLiteral("unitconverter"),
        QStringLiteral("krunner_unitconverter"),
        QStringLiteral("shell"),
        QStringLiteral("krunner_shell"),
        QStringLiteral("krunner_systemsettings"),
        QStringLiteral("systemsettings"),
        // The power rows are the launcher's own.
        QStringLiteral("sessions"),
        QStringLiteral("krunner_sessions"),
        // Files come from Explorer's index, recent files from the xbel file.
        QStringLiteral("baloosearch"),
        QStringLiteral("krunner_baloosearch"),
        QStringLiteral("recentdocuments"),
        QStringLiteral("krunner_recentdocuments"),
    };
    return ids.contains(id);
}
}

Runners::Runners(QObject *backend, QObject *parent)
    : QObject(parent)
    , m_backend(backend)
{
}

Runners::~Runners() = default;

QStringList Runners::allowedRunners() const
{
    // krunnerrc [Plugins] <pluginId>Enabled, as Plasma's KCM writes it; a
    // plugin without a key follows its own default.
    const KConfigGroup plugins(KSharedConfig::openConfig(QStringLiteral("krunnerrc")), QStringLiteral("Plugins"));
    QStringList allowed;
    const auto all = KRunner::RunnerManager::runnerMetaDataList();
    for (const KPluginMetaData &data : all) {
        const QString id = data.pluginId();
        if (drawnByLauncher(id)) {
            continue;
        }
        if (plugins.readEntry(id + QLatin1String("Enabled"), data.isEnabledByDefault())) {
            allowed << id;
        }
    }
    return allowed;
}

KRunner::RunnerManager *Runners::manager()
{
    if (m_manager) {
        return m_manager;
    }
    const KConfigGroup plugins(KSharedConfig::openConfig(QStringLiteral("krunnerrc")), QStringLiteral("Plugins"));
    const KConfigGroup state(KSharedConfig::openStateConfig(QStringLiteral("atlas-launcherstaterc")), QStringLiteral("General"));
    m_manager = new KRunner::RunnerManager(plugins, state, this);
    // What the user typed is not kept.
    m_manager->setHistoryEnabled(false);
    m_manager->setAllowedRunners(allowedRunners());
    connect(m_manager, &KRunner::RunnerManager::matchesChanged, this, &Runners::onMatches);
    m_dirty = false;
    if (m_session) {
        // The panel is open: the session the show asked for starts now.
        m_manager->setupMatchSession();
    }
    return m_manager;
}

void Runners::prewarm()
{
    manager();
}

void Runners::setupMatchSession()
{
    if (m_session) {
        return;
    }
    m_session = true;
    if (!m_manager) {
        // Created at the first non-empty query.
        return;
    }
    if (m_dirty) {
        m_dirty = false;
        m_manager->setAllowedRunners(allowedRunners());
        m_manager->reloadConfiguration();
    }
    m_manager->setupMatchSession();
}

void Runners::matchSessionComplete()
{
    m_matches.clear();
    if (!m_session) {
        return;
    }
    m_session = false;
    if (m_manager) {
        m_manager->reset();
        m_manager->matchSessionComplete();
    }
}

void Runners::reload()
{
    // Never under a live session: it applies at the next setupMatchSession.
    // Without a manager the config is read when it is created.
    if (m_manager) {
        m_dirty = true;
    }
}

void Runners::searchStarted(const QString &text)
{
    m_matches.clear();
    m_serial = m_backend->property("serial").toInt();
    if (text.trimmed().isEmpty()) {
        if (m_manager) {
            m_manager->reset();
        }
        return;
    }
    // Created here at the first query. When the panel is open its session
    // starts with the manager; a query typed before the panel's show signal
    // (Search with text) starts one now.
    KRunner::RunnerManager *runnerManager = manager();
    if (!m_session) {
        setupMatchSession();
    }
    runnerManager->launchQuery(text.left(kMaxQueryLength));
}

void Runners::onMatches(const QList<KRunner::QueryMatch> &matches)
{
    // Late matches after the panel hid belong to nothing.
    if (!m_session) {
        return;
    }
    m_matches.clear();
    QVariantList list;
    const qsizetype count = std::min<qsizetype>(matches.size(), kMaxMatches);
    list.reserve(count);
    for (qsizetype i = 0; i < count; ++i) {
        const KRunner::QueryMatch &match = matches.at(i);
        if (!match.isValid() || !match.runner()) {
            continue;
        }
        const QString runnerId = match.runner()->id();
        const QString matchId = match.id();
        m_matches.insert({runnerId, matchId}, match);
        QVariantMap map;
        map.insert(QStringLiteral("runnerId"), runnerId);
        map.insert(QStringLiteral("matchId"), matchId);
        map.insert(QStringLiteral("text"), match.text());
        map.insert(QStringLiteral("subtext"), match.subtext());
        map.insert(QStringLiteral("icon"), match.iconName());
        map.insert(QStringLiteral("relevance"), double(match.relevance()));
        list.append(map);
    }
    QMetaObject::invokeMethod(m_backend, "mergeRunner", Q_ARG(QVariant, QVariant(m_serial)), Q_ARG(QVariant, QVariant(list)));
}

std::optional<KRunner::QueryMatch> Runners::find(const QString &runnerId, const QString &matchId) const
{
    const auto it = m_matches.constFind({runnerId, matchId});
    if (it == m_matches.constEnd()) {
        return std::nullopt;
    }
    return *it;
}

bool Runners::run(const KRunner::QueryMatch &match)
{
    // True means the window should close, not that the match worked.
    if (!m_manager) {
        return false;
    }
    return m_manager->run(match);
}
