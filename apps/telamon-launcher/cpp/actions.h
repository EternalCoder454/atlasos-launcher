// What QML calls directly: the footer's power menu, the context menus and the
// user tile (docs/DESIGN.md, "Interfaces"). The work is Executor's; this adds
// the capabilities and the user's name and picture, read asynchronously at
// start with timeouts.
#pragma once

#include <QObject>
#include <QString>

class Executor;

class Actions : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool canSuspend READ canSuspend NOTIFY changed)
    Q_PROPERTY(bool canHibernate READ canHibernate NOTIFY changed)
    Q_PROPERTY(bool canSwitchUser READ canSwitchUser NOTIFY changed)
    Q_PROPERTY(QString userName READ userName NOTIFY changed)
    Q_PROPERTY(QString userIcon READ userIcon NOTIFY changed)

public:
    // `backend` is the Rust Backend (refreshed after Remove from Recent).
    Actions(Executor *executor, QObject *backend, QObject *parent = nullptr);

    bool canSuspend() const { return m_canSuspend; }
    bool canHibernate() const { return m_canHibernate; }
    bool canSwitchUser() const { return m_canSwitchUser; }
    QString userName() const;
    QString userIcon() const { return m_userIcon; }

    // Starts the asynchronous reads (logind, the seat, AccountsService).
    void load();

    Q_INVOKABLE void power(const QString &name);
    Q_INVOKABLE void openContainingFolder(const QString &uri);
    Q_INVOKABLE void copyText(const QString &text);
    Q_INVOKABLE void removeRecent(const QString &uri);
    Q_INVOKABLE void openAppSettings(const QString &desktopId);
    Q_INVOKABLE void openUserSettings();
    Q_INVOKABLE void openSettingsApp();
    Q_INVOKABLE void openFiles();
    Q_INVOKABLE void uninstall(const QString &flatpakId);
    Q_INVOKABLE void launchAction(const QString &desktopId, const QString &actionId);
    Q_INVOKABLE void editApplications();

Q_SIGNALS:
    void changed();
    // logind's and the seat's answers are in (or timed out): `start` can run.
    void capabilitiesReady();
    // A call failed, by kind (SessionCall, SettingsCall, BadArgument, ...);
    // the footer shows a plain message.
    void failed(const QString &kind);

private:
    void answered();
    void loadCapability(const char *method, bool Actions::*member);
    void loadSeat();
    void loadUser();

    Executor *m_executor;
    QObject *m_backend;
    bool m_canSuspend = false;
    bool m_canHibernate = false;
    bool m_canSwitchUser = false;
    mutable QString m_userName;
    mutable bool m_userNameRead = false;
    QString m_userIcon;
    int m_waiting = 3;
};
