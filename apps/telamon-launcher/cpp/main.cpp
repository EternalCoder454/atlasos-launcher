// Starts the launcher: one resident process per session that keeps its panel
// created and hidden, so Meta only has to show it (docs/DESIGN.md, "Form").
// Launch arguments of a second start (`telamon-launcher --search`) go to the
// first through KDBusService.
#include "actions.h"
#include "catalog.h"
#include "dbus.h"
#include "executor.h"
#include "options.h"
#include "panel.h"
#include "runners.h"

#include <telamon/app.h>

#include <KDBusService>
#include <KGlobalAccel>

#include <QAction>
#include <QApplication>
#include <QCommandLineParser>
#include <QDBusConnection>
#include <QDBusError>
#include <QDir>
#include <QLoggingCategory>
#include <QQmlApplicationEngine>
#include <QQuickWindow>
#include <QTimer>
#include <QSGRendererInterface>
#include <QSocketNotifier>
#include <QThreadPool>

#include <cerrno>
#include <cstdint>
#include <csignal>
#include <cstdlib>
#include <memory>

#include <sys/socket.h>
#include <unistd.h>

Q_LOGGING_CATEGORY(lcMain, "telamon.launcher", QtInfoMsg)

// Defined in src/lib.rs: the backend and its five list models, parentless.
// This file owns them: the engine and panel go first, then the backend, then
// the models.
struct LauncherObjects {
    void *backend;
    void *models[5];
};
extern "C" LauncherObjects telamon_launcher_objects_new();
// Defined in src/lib.rs: moves the files of the name before the rename.
extern "C" int32_t telamon_launcher_migrate_user_files();

namespace
{
const QString kComponent = QStringLiteral("net.eterneon.telamon.launcher");
// The KGlobalAccel component before the rename (Atlas Launcher).
const QString kLegacyComponent = QStringLiteral("net.eterneon.atlas.launcher");

// SIGTERM (logout, `systemctl --user stop`), SIGINT and SIGHUP end the event
// loop like a normal quit, so the shutdown below runs and the backend saves
// its files. The handler only writes a byte; the loop reads it.
int signalPipe[2] = {-1, -1};

void onQuitSignal(int)
{
    const int saved = errno;
    const char byte = 1;
    [[maybe_unused]] const auto written = ::write(signalPipe[0], &byte, 1);
    errno = saved;
}

void quitOnSignals(QCoreApplication &app)
{
    if (::socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0, signalPipe) != 0) {
        qCWarning(lcMain) << "no signal pipe, errno" << errno;
        return;
    }
    auto *notifier = new QSocketNotifier(signalPipe[1], QSocketNotifier::Read, &app);
    QObject::connect(notifier, &QSocketNotifier::activated, &app, [notifier] {
        notifier->setEnabled(false);
        qCInfo(lcMain) << "quit signal";
        QCoreApplication::quit();
    });
    struct sigaction action = {};
    action.sa_handler = onQuitSignal;
    sigemptyset(&action.sa_mask);
    action.sa_flags = SA_RESTART;
    for (const int sig : {SIGTERM, SIGINT, SIGHUP}) {
        ::sigaction(sig, &action, nullptr);
    }
}

void addOptions(QCommandLineParser &parser)
{
    parser.setApplicationDescription(QStringLiteral("The Start menu and search of Telamon OS."));
    parser.addHelpOption();
    parser.addVersionOption();
    parser.addOption({QStringLiteral("daemon"), QStringLiteral("Start hidden (the session's user unit uses this).")});
    parser.addOption({QStringLiteral("start"), QStringLiteral("Open Start.")});
    parser.addOption({QStringLiteral("search"), QStringLiteral("Open Search.")});
    parser.addOption({QStringLiteral("hide"), QStringLiteral("Hide the launcher.")});
    parser.addPositionalArgument(QStringLiteral("text"), QStringLiteral("With --search: the text to type in (nothing runs)."), QStringLiteral("[text...]"));
}

// What a start's arguments ask for, from this process's own command line or a
// second start's (forwarded by KDBusService). `second`: a start without any
// flag (the desktop file launched again) opens Start; `--daemon` alone does
// nothing, as the first start's does.
void act(const QCommandLineParser &parser, Panel &panel, bool second)
{
    if (parser.isSet(QStringLiteral("hide"))) {
        panel.hide();
    } else if (parser.isSet(QStringLiteral("search"))) {
        panel.showSearch(parser.positionalArguments().join(QLatin1Char(' ')).left(256));
    } else if (parser.isSet(QStringLiteral("start"))) {
        panel.showStart();
    } else if (second && !parser.isSet(QStringLiteral("daemon"))) {
        panel.showStart();
    }
}

// KGlobalAccel keeps the user's own bindings, so a set that did not take is
// not an error; the log says which actions ended up without keys.
void registerShortcut(QAction *action, const QList<QKeySequence> &keys)
{
    KGlobalAccel::self()->setDefaultShortcut(action, keys);
    const bool set = KGlobalAccel::self()->setShortcut(action, keys);
    const QList<QKeySequence> active = KGlobalAccel::self()->shortcut(action);
    if (!set || active.isEmpty()) {
        qCWarning(lcMain) << "shortcut not active for action" << action->objectName() << "- active keys:" << active.size();
    }
}

// What the user changed in Settings > Shortcuts for the old component
// (net.eterneon.atlas.launcher) carries over to the same action of the new
// one, once: only when the new component has no keys of its own for it yet.
// The caller clears the old component when both actions are done.
QList<QKeySequence> legacyKeys(const QString &actionId)
{
    const QList<QKeySequence> keys = KGlobalAccel::self()->globalShortcut(kLegacyComponent, actionId);
    return KGlobalAccel::self()->globalShortcut(kComponent, actionId).isEmpty() ? keys : QList<QKeySequence>{};
}

void carryOver(QAction *action, const QList<QKeySequence> &old, const QList<QKeySequence> &defaults)
{
    if (!old.isEmpty() && old != defaults) {
        KGlobalAccel::self()->setShortcut(action, old, KGlobalAccel::NoAutoloading);
    }
}

void registerShortcuts(Panel *panel)
{
    // KGlobalAccel keeps the user's own bindings (Autoloading); these are
    // the defaults. Nothing asks the user, unlike the GlobalShortcuts portal.
    auto *search = new QAction(QObject::tr("Search"), panel);
    search->setObjectName(QStringLiteral("toggle-search"));
    search->setProperty("componentName", kComponent);
    search->setProperty("componentDisplayName", QStringLiteral("Telamon Launcher"));
    const QList<QKeySequence> searchKeys{QKeySequence(Qt::ALT | Qt::Key_Space), QKeySequence(Qt::ALT | Qt::Key_F2)};
    const QList<QKeySequence> oldSearch = legacyKeys(search->objectName());
    registerShortcut(search, searchKeys);
    carryOver(search, oldSearch, searchKeys);
    QObject::connect(search, &QAction::triggered, panel, [panel] {
        panel->toggleSearch();
    });

    auto *metaS = new QAction(QObject::tr("Search (Meta+S)"), panel);
    metaS->setObjectName(QStringLiteral("toggle-search-meta-s"));
    metaS->setProperty("componentName", kComponent);
    metaS->setProperty("componentDisplayName", QStringLiteral("Telamon Launcher"));
    const QList<QKeySequence> metaSKeys{QKeySequence(Qt::META | Qt::Key_S)};
    const QList<QKeySequence> oldMetaS = legacyKeys(metaS->objectName());
    registerShortcut(metaS, metaSKeys);
    carryOver(metaS, oldMetaS, metaSKeys);
    // The old component is not used any more: clear its entries (a no-op when
    // there are none, and refused while an old process is still running).
    KGlobalAccel::cleanComponent(kLegacyComponent);
    QObject::connect(metaS, &QAction::triggered, panel, [panel] {
        panel->toggleSearch();
    });
}
}

int main(int argc, char *argv[])
{
    telamon_app_init();
    // Drawn on the CPU like the other Telamon apps unless QT_QUICK_BACKEND says
    // otherwise; the P phase measures software against the GPU.
    if (qEnvironmentVariableIsEmpty("QT_QUICK_BACKEND")) {
        QQuickWindow::setGraphicsApi(QSGRendererInterface::Software);
    }
    // The launcher stays running with its panel hidden. KIO jobs (app
    // launches, runner workers) hold QEventLoopLockers, and with the panel
    // hidden the last one ending would otherwise quit the app.
    QApplication::setQuitOnLastWindowClosed(false);
    QCoreApplication::setQuitLockEnabled(false);
    QApplication app(argc, argv);
    telamon_app_ready();
    // The folders of the old name (atlas-launcher) move to the new ones first,
    // once, before anything reads launcher.conf, pinned.list or usage.tsv.
    telamon_launcher_migrate_user_files();

    QCommandLineParser parser;
    addOptions(parser);
    parser.process(app);

    // A second start forwards its arguments here and exits.
    KDBusService service(KDBusService::Unique);

    const LauncherObjects objects = telamon_launcher_objects_new();
    auto *backend = static_cast<QObject *>(objects.backend);
    QObject *models[5];
    for (int i = 0; i < 5; ++i) {
        models[i] = static_cast<QObject *>(objects.models[i]);
    }

    // The providers that feed the backend. Options come first: the engine
    // starts after them.
    Options options(backend);
    Catalog catalog(backend);
    Runners runners(backend);
    Executor executor(backend, &runners);
    Actions actions(&executor, backend);
    QObject::connect(backend, SIGNAL(searchStarted(QString)), &runners, SLOT(searchStarted(QString)));
    // Only krunnerrc concerns the runners, not launcher.conf.
    QObject::connect(&options, &Options::krunnerChanged, &runners, &Runners::reload);

    options.apply();

    // The engine starts once logind and the seat have answered, or after 1 s.
    bool started = false;
    bool sentHibernate = false;
    bool sentSwitchUser = false;
    auto startBackend = [&] {
        if (started) {
            return;
        }
        started = true;
        sentHibernate = actions.canHibernate();
        sentSwitchUser = actions.canSwitchUser();
        QMetaObject::invokeMethod(backend, "start", Q_ARG(bool, sentHibernate), Q_ARG(bool, sentSwitchUser));
    };
    QObject::connect(&actions, &Actions::capabilitiesReady, &app, startBackend);
    // logind or the seat may answer after the 1 s start: tell the backend.
    QObject::connect(&actions, &Actions::changed, &app, [&] {
        if (!started || (sentHibernate == actions.canHibernate() && sentSwitchUser == actions.canSwitchUser())) {
            return;
        }
        sentHibernate = actions.canHibernate();
        sentSwitchUser = actions.canSwitchUser();
        QMetaObject::invokeMethod(backend, "setSession", Q_ARG(bool, sentHibernate), Q_ARG(bool, sentSwitchUser));
    });
    QTimer::singleShot(1000, &app, startBackend);
    actions.load();

    // Heap, so the exit can drop them in order: panel, engine, backend, models.
    auto engine = std::make_unique<QQmlApplicationEngine>();
    engine->setInitialProperties({
        {QStringLiteral("backend"), QVariant::fromValue(backend)},
        {QStringLiteral("results"), QVariant::fromValue(models[0])},
        {QStringLiteral("pins"), QVariant::fromValue(models[1])},
        {QStringLiteral("apps"), QVariant::fromValue(models[2])},
        {QStringLiteral("recentApps"), QVariant::fromValue(models[3])},
        {QStringLiteral("recentFiles"), QVariant::fromValue(models[4])},
        {QStringLiteral("actions"), QVariant::fromValue(&actions)},
        {QStringLiteral("options"), QVariant::fromValue(&options)},
    });
    engine->loadFromModule("net.eterneon.telamon.launcher", "Panel");
    auto destroyAll = [&](std::unique_ptr<Panel> &panelOwner) {
        // The engine (and its window) stops first, while the Panel its QML
        // reads still exists; then the Panel (it holds the window by
        // QPointer), the backend whose models the QML showed, and the models.
        engine.reset();
        panelOwner.reset();
        delete backend;
        for (QObject *model : models) {
            delete model;
        }
    };
    auto *window = engine->rootObjects().isEmpty() ? nullptr : qobject_cast<QQuickWindow *>(engine->rootObjects().constFirst());
    std::unique_ptr<Panel> panel;
    if (!window) {
        qCCritical(lcMain) << "The panel could not be loaded";
        destroyAll(panel);
        return 1;
    }

    // The catalogue is read once the window is up: its first snapshot is not
    // on the way of the first frame.
    QTimer::singleShot(0, &catalog, [&catalog] {
        catalog.rebuild();
    });
    // The runner manager (plugin metadata, no session) is made once, after
    // startup, so the first query does not pay for it.
    QTimer::singleShot(0, &runners, [&runners] {
        runners.prewarm();
    });

    panel = std::make_unique<Panel>(window);
    window->setProperty("panel", QVariant::fromValue(panel.get()));
    executor.attach(panel.get(), window);

    // KRunner's plugins work only while the panel is open.
    QObject::connect(panel.get(), &Panel::aboutToShow, &runners, [&runners] {
        runners.setupMatchSession();
    });
    QObject::connect(panel.get(), &Panel::shownChanged, &runners, [&runners, p = panel.get()] {
        if (!p->isShown()) {
            runners.matchSessionComplete();
        }
    });

    // Launcher1 sits on KDBusService's object, beside org.freedesktop.Application.
    auto *adaptor = new LauncherAdaptor(&service, panel.get());
    // For this release the name before the rename answers too (the dock button
    // and scripts of the old name): net.eterneon.atlas.launcher, with the same
    // methods and Visible at /net/eterneon/atlas/launcher.
    QObject legacyObject;
    new LegacyLauncherAdaptor(&legacyObject, adaptor, panel.get());
    QDBusConnection bus = QDBusConnection::sessionBus();
    if (bus.isConnected()) {
        if (!bus.registerObject(QStringLiteral("/net/eterneon/atlas/launcher"), &legacyObject, QDBusConnection::ExportAdaptors)) {
            qCWarning(lcMain) << "the old D-Bus object could not be registered";
        } else if (!bus.registerService(QStringLiteral("net.eterneon.atlas.launcher"))) {
            qCWarning(lcMain) << "the old D-Bus name is taken:" << bus.lastError().name();
        }
    }
    adaptor->setBackend(backend);
    // ClearHistory replies when the backend has finished (or after 5 s).
    QObject::connect(backend, SIGNAL(historyCleared(bool)), adaptor, SLOT(onHistoryCleared(bool)));
    registerShortcuts(panel.get());

    QObject::connect(&service, &KDBusService::activateRequested, panel.get(), [p = panel.get()](const QStringList &arguments, const QString &) {
        QCommandLineParser forwarded;
        addOptions(forwarded);
        // parse(), not process(): bad arguments from another start must not
        // exit this one.
        if (!arguments.isEmpty() && forwarded.parse(arguments)) {
            act(forwarded, *p, true);
        } else if (arguments.isEmpty()) {
            p->showStart();
        }
    });
    act(parser, *panel, false);

    quitOnSignals(app);
    const int code = app.exec();
    // Catalogue file-time and recent-file jobs post back to objects on this
    // stack, so they must finish before those go. One stuck on a dead mount
    // must not hold the exit: after 5 s the backend still saves its files
    // (destroyAll), and the process ends without running the destructors
    // the stuck job could still reach.
    if (!QThreadPool::globalInstance()->waitForDone(5000)) {
        qCWarning(lcMain) << "background jobs still running at exit:" << QThreadPool::globalInstance()->activeThreadCount();
        destroyAll(panel);
        std::_Exit(code);
    }
    destroyAll(panel);
    return code;
}
