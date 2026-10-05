// net.eterneon.atlas.Launcher1 (docs/DESIGN.md, "Interfaces"). It sits on
// KDBusService's object (/net/eterneon/atlas/launcher), beside
// org.freedesktop.Application. No method runs a result: they open, place and
// hide the panel, and hand over pins.
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
    Q_CLASSINFO("D-Bus Interface", "net.eterneon.atlas.Launcher1")
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
