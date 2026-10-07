// Carries out what the backend asks for (docs/DESIGN.md, "Launching"). Only
// the user's own input in the panel reaches it: no D-Bus method runs a
// result. Nothing is built from a shell string: apps, files and commands go
// through KIO's launch jobs, with the activation token as startup id.
//
// The same calls serve the Actions object QML uses (the footer, the context
// menus), so they all share the token request, the duplicate guard and the
// panel hide.
#pragma once

#include <QDBusMessage>
#include <QObject>
#include <QPointer>
#include <QStringList>
#include <QUrl>
#include <QVariantList>
#include <QVariantMap>

#include <functional>

class Panel;
class QDBusPendingCallWatcher;
class QQuickWindow;
class Runners;

class Executor : public QObject
{
    Q_OBJECT

public:
    // `backend` is the Rust Backend; its signals are connected by name.
    Executor(QObject *backend, Runners *runners, QObject *parent = nullptr);

    // The panel exists after the QML is loaded; until then every call fails
    // with "NotReady".
    void attach(Panel *panel, QQuickWindow *window);

    // --- Shared by the backend's signals and by Actions. Each reports a
    // failure by kind through failed() and returns false. ---
    bool launchApplication(const QString &desktopId, const QString &actionId);
    bool openLocalFile(const QString &uri);
    bool openWebUrl(const QString &url);
    bool startCommand(const QString &executable, const QStringList &args);
    bool startInTerminal(const QString &line);
    // Settings' org.freedesktop.Application: ActivateAction(action, params)
    // when `action` is set, else Activate().
    bool activateSettings(const QString &action, const QVariantList &params);
    bool showInFolder(const QString &uri);
    // `prompt`: Plasma's prompt with its countdown (from search results);
    // else done at once (the power menu).
    bool session(const QString &name, bool prompt);
    bool copy(const QString &text);

    // One asynchronous call with a timeout. Never null; a bus that is down
    // gives a call that has already failed. The caller connects `finished`.
    QDBusPendingCallWatcher *dbusCall(const QDBusMessage &message, bool systemBus);

    // A file URI from outside, checked: a local file, no host (or localhost),
    // no query, no fragment. An invalid QUrl when it fails; else a clean
    // file:/// URL.
    static QUrl checkedFileUrl(const QString &uri);

    // Runs `run(token)` once the activation token is there (or after 200 ms
    // without), then hides the panel. False when one is already pending (the
    // refusal is reported as "Busy") or the panel is not ready.
    bool launchWithToken(const std::function<void(const QString &token)> &run);

Q_SIGNALS:
    // A call failed, by kind only (never what was run).
    void failed(const QString &kind);

public Q_SLOTS:
    // The backend's signals (connected with SIGNAL/SLOT strings).
    void launchApp(const QString &desktopId, const QString &action);
    void openSettings(const QString &link);
    void openFile(const QString &uri);
    void openWeb(const QString &url);
    void copyText(const QString &text);
    void runCommand(const QString &executable, const QStringList &args);
    void runInTerminal(const QString &line);
    void sessionAction(const QString &name);
    void runRunner(const QString &runnerId, const QString &matchId);
    void activated();
    // The backend refused a row itself (its `problem` signal).
    void backendProblem(const QString &kind);

private:
    // What the backend's last launch signal came to, read by activated().
    enum class Outcome {
        None, // nothing launched: the row only needs the panel closed
        Handled, // accepted: the panel hides itself when the launch goes
        Refused, // failed(kind) was emitted: the panel stays open
    };
    void settle(bool accepted);
    void callSettings(const QString &action, const QVariantList &params, const QVariantMap &platform, const QString &token, bool current);
    void launchSettingsApp(const QString &token);
    void openParentFolder(const QUrl &file, const QString &token);
    bool fail(const char *kind);
    void hidePanel();

    QObject *m_backend;
    Runners *m_runners;
    QPointer<Panel> m_panel;
    QPointer<QQuickWindow> m_window;
    bool m_pending = false;
    bool m_sessionPending = false;
    Outcome m_outcome = Outcome::None;
};
