#include "executor.h"

#include "panel.h"
#include "runners.h"
#include "validate.h"

#include <KIO/ApplicationLauncherJob>
#include <KIO/CommandLauncherJob>
#include <KIO/JobUiDelegateFactory>
#include <KIO/OpenUrlJob>
#include <KJobUiDelegate>
#include <KService>
#include <KServiceAction>
#include <KTerminalLauncherJob>
#include <KWaylandExtras>

#include <QClipboard>
#include <QFileInfo>
#include <QDBusConnection>
#include <QDBusPendingCallWatcher>
#include <QDBusPendingReply>
#include <QFutureWatcher>
#include <QGuiApplication>
#include <QLoggingCategory>
#include <QQuickWindow>
#include <QTimer>

#include <atomic>
#include <memory>
#include <utility>

Q_LOGGING_CATEGORY(lcExec, "telamon.launcher.exec", QtInfoMsg)

namespace
{
constexpr int kTokenTimeoutMs = 200;
constexpr int kCallTimeoutMs = 5000;
constexpr int kMaxCommandLength = 4096;
constexpr int kMaxArgs = 256;
constexpr int kMaxLineLength = 4096;
const QString kAppId = QStringLiteral("net.eterneon.telamon.launcher");
// Telamon Settings' desktop file, the fallback when its D-Bus call fails; then
// the one of the name before the rename (the apps moved one by one).
const QString kSettingsDesktopId = QStringLiteral("net.eterneon.telamon.settings.desktop");
const QString kLegacySettingsDesktopId = QStringLiteral("net.eterneon.atlas.settings.desktop");

// The one UI delegate every job gets: errors after the panel hid show as the
// job's own message. Null when KIO has no factory (logged by the caller).
KJobUiDelegate *delegate()
{
    KJobUiDelegate *created = KIO::createDefaultJobUiDelegate(KJobUiDelegate::AutoHandlingEnabled, nullptr);
    static std::atomic<bool> logged{false};
    if (!created && !logged.exchange(true)) {
        qCWarning(lcExec) << "KIO has no UI delegate factory: launch errors after the panel hides will not be shown";
    }
    return created;
}

void watch(KJob *job, const char *what)
{
    QObject::connect(job, &KJob::result, job, [what](KJob *finished) {
        if (finished->error() != 0) {
            // The error code only: the text can name what ran.
            qCWarning(lcExec) << what << "failed, code" << finished->error();
        }
    });
}

}

Executor::Executor(QObject *backend, Runners *runners, QObject *parent)
    : QObject(parent)
    , m_backend(backend)
    , m_runners(runners)
{
    const struct {
        const char *signal;
        const char *slot;
    } links[] = {
        {SIGNAL(launchApp(QString, QString)), SLOT(launchApp(QString, QString))},
        {SIGNAL(openSettings(QString)), SLOT(openSettings(QString))},
        {SIGNAL(openFile(QString)), SLOT(openFile(QString))},
        {SIGNAL(openWeb(QString)), SLOT(openWeb(QString))},
        {SIGNAL(copyText(QString)), SLOT(copyText(QString))},
        {SIGNAL(runCommand(QString, QStringList)), SLOT(runCommand(QString, QStringList))},
        {SIGNAL(runInTerminal(QString)), SLOT(runInTerminal(QString))},
        {SIGNAL(sessionAction(QString)), SLOT(sessionAction(QString))},
        {SIGNAL(runRunner(QString, QString)), SLOT(runRunner(QString, QString))},
        {SIGNAL(activated()), SLOT(activated())},
        {SIGNAL(problem(QString)), SLOT(backendProblem(QString))},
    };
    for (const auto &link : links) {
        if (!connect(m_backend, link.signal, this, link.slot)) {
            qCCritical(lcExec) << "backend signal not connected:" << link.signal;
        }
    }
}

void Executor::attach(Panel *panel, QQuickWindow *window)
{
    m_panel = panel;
    m_window = window;
}

bool Executor::fail(const char *kind)
{
    qCWarning(lcExec) << "failed:" << kind;
    Q_EMIT failed(QString::fromLatin1(kind));
    return false;
}

void Executor::hidePanel()
{
    if (m_panel) {
        m_panel->hide();
    }
}

bool Executor::launchWithToken(const std::function<void(const QString &)> &run)
{
    if (m_pending) {
        qCDebug(lcExec) << "launch refused: another one is pending";
        return fail("Busy");
    }
    if (!m_panel || !m_window) {
        return fail("NotReady");
    }
    m_pending = true;

    auto finish = [this, run](const QString &token) {
        m_pending = false;
        run(token);
        hidePanel();
    };

    if (QGuiApplication::platformName() != QLatin1String("wayland") || !m_window->isVisible()) {
        finish({});
        return true;
    }

    // Whichever of the token and the timer comes first wins; the other is
    // dropped with `state`.
    auto *state = new QObject(this);
    auto done = std::make_shared<bool>(false);
    auto *watcher = new QFutureWatcher<QString>(state);
    auto *timer = new QTimer(state);
    timer->setSingleShot(true);
    auto once = [state, done, finish](const QString &token) {
        if (*done) {
            return;
        }
        *done = true;
        state->deleteLater();
        finish(token);
    };
    connect(watcher, &QFutureWatcherBase::finished, state, [watcher, once] {
        QString token;
        if (!watcher->future().isCanceled() && watcher->future().resultCount() > 0) {
            token = watcher->result();
        }
        once(token);
    });
    connect(timer, &QTimer::timeout, state, [once] {
        qCInfo(lcExec) << "no activation token in time";
        once({});
    });
    timer->start(kTokenTimeoutMs);
    watcher->setFuture(KWaylandExtras::xdgActivationToken(m_window, kAppId));
    return true;
}

QDBusPendingCallWatcher *Executor::dbusCall(const QDBusMessage &message, bool systemBus)
{
    QDBusConnection bus = systemBus ? QDBusConnection::systemBus() : QDBusConnection::sessionBus();
    return new QDBusPendingCallWatcher(bus.asyncCall(message, kCallTimeoutMs), this);
}

bool Executor::launchApplication(const QString &desktopId, const QString &actionId)
{
    // Not just any string: KService takes an absolute path as a storage id and
    // loads whatever desktop file is there (docs/SECURITY.md).
    if (!Validate::desktopId(desktopId)) {
        return fail("BadArgument");
    }
    const KService::Ptr service = KService::serviceByStorageId(desktopId);
    if (!service) {
        return fail("UnknownApp");
    }
    KServiceAction action;
    if (!actionId.isEmpty()) {
        const auto actions = service->actions();
        for (const KServiceAction &candidate : actions) {
            if (candidate.name() == actionId) {
                action = candidate;
                break;
            }
        }
        if (action.name().isEmpty()) {
            return fail("UnknownAction");
        }
    }
    return launchWithToken([this, service, action](const QString &token) {
        auto *job = action.name().isEmpty() ? new KIO::ApplicationLauncherJob(service, this) : new KIO::ApplicationLauncherJob(action, this);
        job->setUiDelegate(delegate());
        job->setStartupId(token.toUtf8());
        watch(job, "app launch");
        job->start();
    });
}

QUrl Executor::checkedFileUrl(const QString &uri)
{
    return Validate::fileUrl(uri);
}

bool Executor::openLocalFile(const QString &uri)
{
    const QUrl url = checkedFileUrl(uri);
    if (!url.isValid()) {
        return fail("BadArgument");
    }
    return launchWithToken([this, url](const QString &token) {
        auto *job = new KIO::OpenUrlJob(url, this);
        job->setUiDelegate(delegate());
        // A file the user clicked is opened, never run.
        job->setRunExecutables(false);
        job->setStartupId(token.toUtf8());
        watch(job, "file open");
        job->start();
    });
}

bool Executor::openWebUrl(const QString &urlText)
{
    const QUrl url = Validate::webUrl(urlText);
    if (!url.isValid()) {
        return fail("BadArgument");
    }
    return launchWithToken([this, url](const QString &token) {
        auto *job = new KIO::OpenUrlJob(url, this);
        job->setUiDelegate(delegate());
        job->setRunExecutables(false);
        job->setStartupId(token.toUtf8());
        watch(job, "web open");
        job->start();
    });
}

bool Executor::startCommand(const QString &executable, const QStringList &args)
{
    // An absolute path, as the PATH scan found it (or Telamon Store's): the
    // job never looks a bare name up on a PATH again.
    if (!Validate::commandPath(executable, kMaxCommandLength) || args.size() > kMaxArgs) {
        return fail("BadArgument");
    }
    return launchWithToken([this, executable, args](const QString &token) {
        auto *job = new KIO::CommandLauncherJob(executable, args, this);
        job->setUiDelegate(delegate());
        job->setStartupId(token.toUtf8());
        watch(job, "command");
        job->start();
    });
}

bool Executor::startInTerminal(const QString &line)
{
    if (line.trimmed().isEmpty() || line.size() > kMaxLineLength || line.contains(QChar(0))) {
        return fail("BadArgument");
    }
    // The one place a typed line is handed on as a line: "Run in Terminal"
    // is the user's own command.
    return launchWithToken([this, line](const QString &token) {
        auto *job = new KTerminalLauncherJob(line, this);
        job->setUiDelegate(delegate());
        job->setStartupId(token.toUtf8());
        watch(job, "terminal");
        job->start();
    });
}

bool Executor::activateSettings(const QString &action, const QVariantList &params)
{
    return launchWithToken([this, action, params](const QString &token) {
        QVariantMap platform;
        if (!token.isEmpty()) {
            platform.insert(QStringLiteral("activation-token"), token);
        }
        callSettings(action, params, platform, token, true);
    });
}

// Settings answers on net.eterneon.telamon.settings; a Settings that has not
// moved to the new name only on net.eterneon.atlas.settings (a moved one
// answers on both). The new name first, then the old, then the app itself.
void Executor::callSettings(const QString &action, const QVariantList &params, const QVariantMap &platform, const QString &token, bool current)
{
    const QString name = current ? QStringLiteral("net.eterneon.telamon.settings") : QStringLiteral("net.eterneon.atlas.settings");
    const QString path = current ? QStringLiteral("/net/eterneon/telamon/settings") : QStringLiteral("/net/eterneon/atlas/settings");
    QDBusMessage message;
    if (action.isEmpty()) {
        message = QDBusMessage::createMethodCall(name, path, QStringLiteral("org.freedesktop.Application"), QStringLiteral("Activate"));
        message << platform;
    } else {
        message = QDBusMessage::createMethodCall(name, path, QStringLiteral("org.freedesktop.Application"), QStringLiteral("ActivateAction"));
        message << action << params << platform;
    }
    auto *call = dbusCall(message, false);
    connect(call, &QDBusPendingCallWatcher::finished, this, [this, token, action, params, platform, current](QDBusPendingCallWatcher *w) {
        w->deleteLater();
        if (!w->isError()) {
            return;
        }
        if (current) {
            qCInfo(lcExec) << "settings call failed on the new name:" << w->error().name() << "- trying the old";
            callSettings(action, params, platform, token, false);
            return;
        }
        // The panel is gone by now: start the app itself instead.
        qCWarning(lcExec) << "settings call failed:" << w->error().name() << "- starting the app";
        launchSettingsApp(token);
    });
}

void Executor::launchSettingsApp(const QString &token)
{
    KService::Ptr service = KService::serviceByStorageId(kSettingsDesktopId);
    if (!service) {
        service = KService::serviceByStorageId(kLegacySettingsDesktopId);
    }
    if (!service) {
        fail("SettingsCall");
        return;
    }
    auto *job = new KIO::ApplicationLauncherJob(service, this);
    job->setUiDelegate(delegate());
    job->setStartupId(token.toUtf8());
    watch(job, "settings app launch");
    job->start();
}

void Executor::openParentFolder(const QUrl &file, const QString &token)
{
    const QUrl folder = QUrl::fromLocalFile(QFileInfo(file.toLocalFile()).path());
    auto *job = new KIO::OpenUrlJob(folder, this);
    job->setUiDelegate(delegate());
    // A folder is opened, never run.
    job->setRunExecutables(false);
    job->setStartupId(token.toUtf8());
    watch(job, "folder open");
    job->start();
}

bool Executor::showInFolder(const QString &uri)
{
    const QUrl url = checkedFileUrl(uri);
    if (!url.isValid()) {
        return fail("BadArgument");
    }
    return launchWithToken([this, url](const QString &token) {
        auto message = QDBusMessage::createMethodCall(QStringLiteral("org.freedesktop.FileManager1"),
                                                      QStringLiteral("/org/freedesktop/FileManager1"),
                                                      QStringLiteral("org.freedesktop.FileManager1"),
                                                      QStringLiteral("ShowItems"));
        message << QStringList{url.toString(QUrl::FullyEncoded)} << token;
        auto *call = dbusCall(message, false);
        connect(call, &QDBusPendingCallWatcher::finished, this, [this, url, token](QDBusPendingCallWatcher *w) {
            w->deleteLater();
            if (w->isError()) {
                // No file manager answered: open the folder that holds it.
                qCWarning(lcExec) << "file manager call failed:" << w->error().name() << "- opening the folder";
                openParentFolder(url, token);
            }
        });
    });
}

bool Executor::session(const QString &name, bool prompt)
{
    struct Call {
        const char *service;
        const char *path;
        const char *interface;
        const char *method;
        bool system;
        bool withArg;
    };
    Call call{};
    if (name == QLatin1String("lock")) {
        call = {"org.freedesktop.ScreenSaver", "/ScreenSaver", "org.freedesktop.ScreenSaver", "Lock", false, false};
    } else if (name == QLatin1String("sleep")) {
        call = {"org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", "Suspend", true, true};
    } else if (name == QLatin1String("hibernate")) {
        call = {"org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", "Hibernate", true, true};
    } else if (name == QLatin1String("switch-user")) {
        call = {"org.kde.ksmserver", "/KSMServer", "org.kde.KSMServerInterface", "openSwitchUserDialog", false, false};
    } else if (name == QLatin1String("log-out")) {
        call = prompt ? Call{"org.kde.LogoutPrompt", "/LogoutPrompt", "org.kde.LogoutPrompt", "promptLogout", false, false}
                      : Call{"org.kde.Shutdown", "/Shutdown", "org.kde.Shutdown", "logout", false, false};
    } else if (name == QLatin1String("restart")) {
        call = prompt ? Call{"org.kde.LogoutPrompt", "/LogoutPrompt", "org.kde.LogoutPrompt", "promptReboot", false, false}
                      : Call{"org.kde.Shutdown", "/Shutdown", "org.kde.Shutdown", "logoutAndReboot", false, false};
    } else if (name == QLatin1String("shut-down")) {
        call = prompt ? Call{"org.kde.LogoutPrompt", "/LogoutPrompt", "org.kde.LogoutPrompt", "promptShutDown", false, false}
                      : Call{"org.kde.Shutdown", "/Shutdown", "org.kde.Shutdown", "logoutAndShutdown", false, false};
    } else {
        return fail("BadArgument");
    }
    if (m_sessionPending) {
        qCDebug(lcExec) << "session call refused: one is pending";
        return fail("Busy");
    }
    auto message = QDBusMessage::createMethodCall(QString::fromLatin1(call.service),
                                                  QString::fromLatin1(call.path),
                                                  QString::fromLatin1(call.interface),
                                                  QString::fromLatin1(call.method));
    if (call.withArg) {
        // Interactive: polkit may ask the user.
        message << true;
    }
    m_sessionPending = true;
    auto *watcher = dbusCall(message, call.system);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *w) {
        w->deleteLater();
        m_sessionPending = false;
        if (w->isError()) {
            qCWarning(lcExec) << "session call failed:" << w->error().name();
            Q_EMIT failed(QStringLiteral("SessionCall"));
            return;
        }
        hidePanel();
    });
    return true;
}

bool Executor::copy(const QString &text)
{
    if (QClipboard *clipboard = QGuiApplication::clipboard()) {
        clipboard->setText(text);
    } else {
        return fail("NoClipboard");
    }
    hidePanel();
    return true;
}

void Executor::settle(bool accepted)
{
    m_outcome = accepted ? Outcome::Handled : Outcome::Refused;
}

void Executor::launchApp(const QString &desktopId, const QString &action)
{
    settle(launchApplication(desktopId, action));
}

void Executor::openSettings(const QString &link)
{
    // A deep link is a short id like "bluetooth" or "users/accounts".
    if (!Validate::settingsLink(link)) {
        settle(fail("BadArgument"));
        return;
    }
    settle(activateSettings(QStringLiteral("open"), {link}));
}

void Executor::openFile(const QString &uri)
{
    settle(openLocalFile(uri));
}

void Executor::openWeb(const QString &url)
{
    settle(openWebUrl(url));
}

void Executor::copyText(const QString &text)
{
    // copy() hides the panel itself, or reports why not.
    settle(copy(text));
}

void Executor::runCommand(const QString &executable, const QStringList &args)
{
    settle(startCommand(executable, args));
}

void Executor::runInTerminal(const QString &line)
{
    settle(startInTerminal(line));
}

void Executor::sessionAction(const QString &name)
{
    settle(session(name, true));
}

void Executor::runRunner(const QString &runnerId, const QString &matchId)
{
    // Looked up now: the panel's hide drops the matches.
    const auto match = m_runners->find(runnerId, matchId);
    if (!match) {
        // The query moved on under the click.
        settle(fail("StaleResult"));
        return;
    }
    if (!match->runner() || !m_runners->ready()) {
        settle(fail("RunnerFailed"));
        return;
    }
    // Runner rows run at once, with no activation token to wait for. The
    // return value only says whether the window should close; it is not a
    // success flag, so a false keeps the panel open.
    const bool close = m_runners->run(*match);
    m_outcome = Outcome::Handled;
    if (close) {
        hidePanel();
    }
}

void Executor::activated()
{
    // Every row that launches hides the panel itself once its token is
    // there (hiding now would cost the token). A refused row keeps the
    // panel open, with failed(kind) already sent. This covers a row that ran
    // nothing to launch.
    const Outcome outcome = std::exchange(m_outcome, Outcome::None);
    if (outcome == Outcome::None && !m_pending && !m_sessionPending) {
        hidePanel();
    }
}

void Executor::backendProblem(const QString &kind)
{
    m_outcome = Outcome::Refused;
    qCWarning(lcExec) << "backend problem:" << kind.left(64);
    Q_EMIT failed(kind.left(64));
}
