#include "catalog.h"

#include <KApplicationTrader>
#include <KConfigGroup>
#include <KService>
#include <KServiceAction>
#include <KSharedConfig>
#include <KSycoca>

#include <QDateTime>
#include <QFile>
#include <QFileInfo>
#include <QLoggingCategory>
#include <QMetaObject>
#include <QStandardPaths>
#include <QThreadPool>
#include <QVariantList>
#include <QVariantMap>

Q_LOGGING_CATEGORY(lcCatalog, "telamon.launcher.catalog", QtInfoMsg)

// Defined in src/lib.rs: whether an absolute icon path is a non-empty regular
// file of at most 16 MiB that is not on /proc, /sys or another pseudo file
// system (telamon_launcher_core::text::icon_file_ok).
extern "C" bool telamon_launcher_icon_ok(const char *path);

namespace
{
// A hostile desktop file cannot make the snapshot unbounded; the Rust side
// validates and caps again.
constexpr int kMaxApps = 20000;

// The icon of a catalogue entry as the panel may load it: a theme name as it
// is, an absolute path only when the core says it is a plain file (links
// followed, as icon themes use them); "" otherwise, which the panel shows as
// the generic icon. A pipe named by `Icon=` made the image loader wait in
// open() on the GUI thread for good (measured: the launcher stopped
// answering); a device or a huge file would be read into memory whole
// (docs/SECURITY.md). Stats, no open. Off the GUI thread (the caller is on the
// pool).
QString vettedIcon(const QString &icon)
{
    if (!icon.startsWith(QLatin1Char('/'))) {
        return icon;
    }
    return telamon_launcher_icon_ok(QFile::encodeName(icon).constData()) ? icon : QString();
}

// The binary's file name in an Exec line: skips `env` and VAR=value words.
QString execName(const QString &exec)
{
    const QStringList words = exec.split(QLatin1Char(' '), Qt::SkipEmptyParts);
    for (const QString &word : words) {
        if (word == QLatin1String("env") || (word.contains(QLatin1Char('=')) && !word.startsWith(QLatin1Char('/')))) {
            continue;
        }
        QString name = word;
        if (name.size() >= 2 && (name.startsWith(QLatin1Char('"')) || name.startsWith(QLatin1Char('\'')))) {
            name = name.mid(1);
        }
        return QFileInfo(name).fileName();
    }
    return {};
}

QVariantMap appMap(const KService &service)
{
    QStringList actionIds;
    QStringList actionNames;
    QStringList actionIcons;
    const auto actions = service.actions();
    for (const KServiceAction &action : actions) {
        if (action.noDisplay() || action.name().isEmpty()) {
            continue;
        }
        actionIds << action.name();
        actionNames << action.text();
        actionIcons << action.icon();
    }
    QVariantMap map;
    map.insert(QStringLiteral("desktopId"), service.storageId());
    // A desktop file the launcher wrote for a rename (Rename App, so the dock
    // and menus show the name too) says what the app's own name was: the
    // catalogue keeps that as the app's, and names.conf puts the user's on
    // top, as before.
    // The marker means something only in the user's own applications folder,
    // where the launcher writes; in any other desktop file it is just a key.
    const bool marked = service.property<QString>(QStringLiteral("X-Telamon-Renamed")) == QLatin1String("true");
    const bool renamed = marked && service.entryPath().startsWith(QStandardPaths::writableLocation(QStandardPaths::GenericDataLocation) + QLatin1String("/applications/"));
    const QString original = renamed ? service.property<QString>(QStringLiteral("X-Telamon-Original-Name")) : QString();
    map.insert(QStringLiteral("name"), original.isEmpty() ? service.name() : original);
    map.insert(QStringLiteral("genericName"), service.genericName());
    map.insert(QStringLiteral("comment"), service.comment());
    map.insert(QStringLiteral("icon"), service.icon());
    map.insert(QStringLiteral("execName"), execName(service.exec()));
    map.insert(QStringLiteral("flatpakId"), service.property<QString>(QStringLiteral("X-Flatpak")));
    map.insert(QStringLiteral("keywords"), service.keywords());
    map.insert(QStringLiteral("categories"), service.categories());
    map.insert(QStringLiteral("actionIds"), actionIds);
    map.insert(QStringLiteral("actionNames"), actionNames);
    map.insert(QStringLiteral("actionIcons"), actionIcons);
    // "installed" is added once the file times are known.
    return map;
}

QString storageIdOf(const KService::Ptr &service)
{
    return service ? service->storageId() : QString();
}
}

Catalog::Catalog(QObject *backend, QObject *parent)
    : QObject(parent)
    , m_backend(backend)
{
    // Installs come in bursts (one dnf transaction, one flatpak update).
    m_debounce.setSingleShot(true);
    m_debounce.setInterval(500);
    connect(&m_debounce, &QTimer::timeout, this, &Catalog::rebuild);
    connect(KSycoca::self(), &KSycoca::databaseChanged, &m_debounce, qOverload<>(&QTimer::start));
}

void Catalog::rebuild()
{
    const KService::List services = KApplicationTrader::query([](const KService::Ptr &service) {
        return service->isApplication() && !service->noDisplay() && service->showInCurrentDesktop() && service->showOnCurrentPlatform();
    });

    QVariantList apps;
    QStringList paths;
    apps.reserve(qMin<qsizetype>(services.size(), kMaxApps));
    paths.reserve(apps.capacity());
    for (const KService::Ptr &service : services) {
        if (apps.size() >= kMaxApps) {
            break;
        }
        if (!service || service->storageId().isEmpty()) {
            continue;
        }
        apps.append(appMap(*service));
        paths.append(service->entryPath());
    }

    QVariantMap preferred;
    const QString browser = storageIdOf(KApplicationTrader::preferredService(QStringLiteral("x-scheme-handler/https")));
    const QString files = storageIdOf(KApplicationTrader::preferredService(QStringLiteral("inode/directory")));
    QString terminal = KConfigGroup(KSharedConfig::openConfig(QStringLiteral("kdeglobals")), QStringLiteral("General")).readEntry("TerminalService", QString());
    if (terminal.isEmpty() || !KService::serviceByStorageId(terminal)) {
        terminal = storageIdOf(KService::serviceByStorageId(QStringLiteral("org.kde.konsole.desktop")));
    }
    if (!browser.isEmpty()) {
        preferred.insert(QStringLiteral("preferred://browser"), browser);
    }
    if (!files.isEmpty()) {
        preferred.insert(QStringLiteral("preferred://filemanager"), files);
    }
    if (!terminal.isEmpty()) {
        preferred.insert(QStringLiteral("preferred://terminal"), terminal);
    }

    // The stat of every desktop file is disk I/O: off the GUI thread, and
    // only for files not seen before.
    const quint64 generation = ++m_generation;
    QThreadPool::globalInstance()->start([this, generation, apps = std::move(apps), paths = std::move(paths), preferred = std::move(preferred), known = m_installed]() mutable {
        QHash<QString, qint64> installed;
        installed.reserve(paths.size());
        for (qsizetype i = 0; i < apps.size(); ++i) {
            const QString &path = paths.at(i);
            qint64 time = 0;
            const auto hit = known.constFind(path);
            if (hit != known.constEnd()) {
                time = *hit;
            } else if (!path.isEmpty()) {
                time = QFileInfo(path).lastModified().toSecsSinceEpoch();
            }
            installed.insert(path, time);
            QVariantMap map = apps.at(i).toMap();
            map.insert(QStringLiteral("installed"), time);
            map.insert(QStringLiteral("icon"), vettedIcon(map.value(QStringLiteral("icon")).toString()));
            QStringList actionIcons = map.value(QStringLiteral("actionIcons")).toStringList();
            for (QString &actionIcon : actionIcons) {
                actionIcon = vettedIcon(actionIcon);
            }
            map.insert(QStringLiteral("actionIcons"), actionIcons);
            apps[i] = map;
        }
        // `this` is the context: the call is dropped if the catalogue is gone.
        QMetaObject::invokeMethod(
            this,
            [this, generation, apps = std::move(apps), preferred = std::move(preferred), installed = std::move(installed)]() {
                if (generation != m_generation) {
                    return;
                }
                m_installed = installed;
                qCInfo(lcCatalog) << "catalogue" << apps.size() << "apps";
                QMetaObject::invokeMethod(m_backend, "setApps", Q_ARG(QVariant, QVariant(apps)), Q_ARG(QVariant, QVariant(preferred)));
            },
            Qt::QueuedConnection);
    });
}
