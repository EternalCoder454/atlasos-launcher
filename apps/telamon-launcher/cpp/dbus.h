// net.eterneon.telamon.Launcher1 (docs/DESIGN.md, "Interfaces"). It sits on
// KDBusService's object (/net/eterneon/telamon/launcher), beside
// org.freedesktop.Application. No method runs a result: they open, place and
// hide the panel, and hand over pins.
//
// For this release the name before the rename answers too: the same methods
// and the Visible property as net.eterneon.atlas.Launcher1, on the bus name
// net.eterneon.atlas.launcher at /net/eterneon/atlas/launcher
// (LegacyLauncherAdaptor), because the dock button of the old name and a
// shortcut or script calling it are not updated at once.
#pragma once

#include <QDBusAbstractAdaptor>
#include <QDBusContext>
#include <QDBusMessage>
#include <QList>
#include <QStringList>
#include <QTimer>
#include <QVariantMap>

class Panel;

class LauncherAdaptor : public QDBusAbstractAdaptor, protected QDBusContext
{
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "net.eterneon.telamon.Launcher1")
    Q_PROPERTY(bool Visible READ visible)

public:
    LauncherAdaptor(QObject *parent, Panel *panel);

    bool visible() const;

Q_SIGNALS:
    // ImportPins and ClearHistory are carried out by the backend.
    void importPinsRequested(const QStringList &ids);
    void clearHistoryRequested();

public Q_SLOTS:
    void ToggleStart();
    void ToggleStart(const QVariantMap &options);
    void ToggleSearch();
    void Show(const QString &mode, const QString &query, const QVariantMap &platformData);
    void Hide();
    void SetDockAnchor(const QVariantMap &options);
    void ImportPins(const QStringList &ids);
    void ClearHistory();

    // ClearHistory for a call that came in on another adaptor (the legacy
    // one): the reply goes out when the backend has finished.
    void queueClearHistory(const QDBusMessage &call);

private Q_SLOTS:
    // The backend's answer to clearHistoryRequested. Private, so the
    // adaptor does not export it as a D-Bus method.
    void onHistoryCleared(bool ok);

private:
    void emitVisibleChanged();
    void replyToClearHistory(bool ok, const QString &error);

    Panel *m_panel;
    // ClearHistory calls waiting for the backend (delayed replies), and the
    // single-shot that gives them an error after 5 s. It runs only while a
    // call is waiting.
    QList<QDBusMessage> m_clearPending;
    QTimer m_clearTimeout;
};

// net.eterneon.atlas.Launcher1: the interface of the name before the rename,
// forwarding every call to the LauncherAdaptor (same checks, same effects).
class LegacyLauncherAdaptor : public QDBusAbstractAdaptor, protected QDBusContext
{
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "net.eterneon.atlas.Launcher1")
    Q_PROPERTY(bool Visible READ visible)

public:
    LegacyLauncherAdaptor(QObject *parent, LauncherAdaptor *target, Panel *panel);

    bool visible() const;

public Q_SLOTS:
    void ToggleStart();
    void ToggleStart(const QVariantMap &options);
    void ToggleSearch();
    void Show(const QString &mode, const QString &query, const QVariantMap &platformData);
    void Hide();
    void SetDockAnchor(const QVariantMap &options);
    void ImportPins(const QStringList &ids);
    void ClearHistory();

private:
    void emitVisibleChanged();

    LauncherAdaptor *m_target;
    Panel *m_panel;
};
