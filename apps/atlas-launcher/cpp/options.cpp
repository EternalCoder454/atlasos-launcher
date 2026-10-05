#include "options.h"

#include <KConfigGroup>

#include <QLocale>
#include <QMetaObject>
#include <QVariantMap>

namespace
{
// A runner's switch under its real plugin id. Plasma 6 writes
// `<pluginId>Enabled` with the installed id: `krunner_services`,
// `krunner_shell`, `krunner_systemsettings`, but the bare `calculator` and
// `unitconverter`. The `krunner_` spelling is tried first for the bare name
// the other way round too, so an id change upstream keeps working.
bool runnerEnabled(const KConfigGroup &plugins, const QString &bare, bool fallback)
{
    const QString prefixed = QLatin1String("krunner_") + bare;
    const bool bareFirst = bare == QLatin1String("calculator") || bare == QLatin1String("unitconverter");
    const QString order[2] = {bareFirst ? bare : prefixed, bareFirst ? prefixed : bare};
    for (const QString &id : order) {
        const QString key = id + QLatin1String("Enabled");
        if (plugins.hasKey(key)) {
            return plugins.readEntry(key, fallback);
        }
    }
    return fallback;
}

// Engines the Rust side knows; anything else it replaces by the default.
constexpr int kMaxEngineLength = 32;
}

Options::Options(QObject *backend, QObject *parent)
    : QObject(parent)
    , m_backend(backend)
    , m_launcher(KSharedConfig::openConfig(QStringLiteral("atlas-launcher/launcher.conf")))
    , m_krunner(KSharedConfig::openConfig(QStringLiteral("krunnerrc")))
{
    m_launcherWatcher = KConfigWatcher::create(m_launcher);
    m_krunnerWatcher = KConfigWatcher::create(m_krunner);
    connect(m_launcherWatcher.data(), &KConfigWatcher::configChanged, this, [this] {
        reload(false);
    });
    connect(m_krunnerWatcher.data(), &KConfigWatcher::configChanged, this, [this] {
        reload(true);
    });
}

bool Options::showRecent() const
{
    return m_showRecent;
}

bool Options::showRecentFiles() const
{
    return m_showRecentFiles;
}

void Options::reload(bool krunner)
{
    if (krunner) {
        m_krunner->reparseConfiguration();
    } else {
        m_launcher->reparseConfiguration();
    }
    apply();
    Q_EMIT changed();
    if (krunner) {
        Q_EMIT krunnerChanged();
    }
}

void Options::apply()
{
    const KConfigGroup search(m_launcher, QStringLiteral("Search"));
    const KConfigGroup start(m_launcher, QStringLiteral("Start"));
    const KConfigGroup plugins(m_krunner, QStringLiteral("Plugins"));

    m_showRecent = start.readEntry("ShowRecent", true);
    m_showRecentFiles = start.readEntry("ShowRecentFiles", true);

    QString engine = search.readEntry("WebSearchEngine", QStringLiteral("duckduckgo"));
    if (engine.size() > kMaxEngineLength) {
        engine = QStringLiteral("duckduckgo");
    }

    QVariantMap map;
    map.insert(QStringLiteral("apps"), runnerEnabled(plugins, QStringLiteral("services"), true));
    map.insert(QStringLiteral("settings"), runnerEnabled(plugins, QStringLiteral("systemsettings"), true));
    map.insert(QStringLiteral("calculator"), runnerEnabled(plugins, QStringLiteral("calculator"), true));
    map.insert(QStringLiteral("units"), runnerEnabled(plugins, QStringLiteral("unitconverter"), true));
    map.insert(QStringLiteral("commands"), runnerEnabled(plugins, QStringLiteral("shell"), true));
    map.insert(QStringLiteral("files"), search.readEntry("FileSearch", true));
    map.insert(QStringLiteral("web"), search.readEntry("WebSearch", true));
    map.insert(QStringLiteral("learn"), search.readEntry("LearnFromUse", true));
    map.insert(QStringLiteral("webEngine"), engine);
    map.insert(QStringLiteral("decimal"), QLocale().decimalPoint() == QLatin1Char(',') ? QStringLiteral(",") : QStringLiteral("."));
    QMetaObject::invokeMethod(m_backend, "setOptions", Q_ARG(QVariant, QVariant(map)));
}
