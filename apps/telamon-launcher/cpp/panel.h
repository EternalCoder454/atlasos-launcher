// The launcher's one window, made a layer-shell surface and kept created
// while hidden, so opening it only maps it (docs/DESIGN.md, "Form").
#pragma once

#include <QObject>
#include <QPointer>
#include <QRect>
#include <QRegion>
#include <QString>
#include <QElapsedTimer>
#include <QTimer>

class QQuickWindow;
class QScreen;

namespace LayerShellQt
{
class Window;
}

class Panel : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QString mode READ mode NOTIFY modeChanged)
    Q_PROPERTY(bool shown READ isShown NOTIFY shownChanged)

public:
    // Where Start opens: the dock button's screen and its rect in global
    // (logical) coordinates. An empty screen name means none known.
    struct DockAnchor {
        QString screen;
        QRect rect;
    };

    explicit Panel(QQuickWindow *window, QObject *parent = nullptr);

    QString mode() const;
    bool isShown() const;

    void setDockAnchor(const DockAnchor &anchor);

    // Opens Start: above the dock button when one is known (this call's
    // anchor, else the last SetDockAnchor), else at the bottom centre of the
    // active screen.
    void showStart(const DockAnchor &anchor = {}, const QString &query = {});
    // Opens Search in the middle of the active screen.
    void showSearch(const QString &query = {});
    void toggleStart(const DockAnchor &anchor = {});
    void toggleSearch();
    Q_INVOKABLE void hide();
    // Sets the blur behind the window again: it follows the window's
    // `blurEnabled`, `cornerRadius` and `blurOffset` (qml/Panel.qml). A no-op
    // while the window is hidden; each show sets it.
    Q_INVOKABLE void refreshBlur();

Q_SIGNALS:
    void modeChanged();
    void shownChanged();
    // Emitted just before the window maps, after the mode is set, so QML can
    // reset its view (set the query, scroll to the top) in the same frame.
    void aboutToShow(const QString &mode, const QString &query);

private:
    void place(const QString &mode, const DockAnchor &anchor);
    void map(const QString &query);
    void updateBlur(bool force = false);
    QScreen *screenNamed(const QString &name) const;

    QPointer<QQuickWindow> m_window;
    LayerShellQt::Window *m_layer = nullptr;
    QString m_mode = QStringLiteral("start");
    DockAnchor m_dock;
    // Single-shot, 60 s after a hide: the only timer while hidden.
    QTimer m_trim;
    // When the window was last mapped; a focus loss within kFocusGraceMs
    // of it is the old mapping's (see the constructor).
    QElapsedTimer m_mappedAt;
    // The time from the start of a show to its first frame handed to the
    // compositor, logged once per show (category debug).
    QElapsedTimer m_showTimer;
    bool m_awaitFrame = false;
    // What was last asked of the compositor, so a repeat is not sent again.
    bool m_blurApplied = false;
    bool m_blurOn = false;
    QRegion m_blurRegion;
    static constexpr qint64 kFocusGraceMs = 500;
};
