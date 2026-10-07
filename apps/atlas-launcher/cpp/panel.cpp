#include "panel.h"

#include <KWindowEffects>
#include <LayerShellQt/Window>

#include <QGuiApplication>
#include <QLoggingCategory>
#include <QPainterPath>
#include <QQuickWindow>
#include <QRegion>
#include <QScreen>

#include <algorithm>

#include <malloc.h>

Q_LOGGING_CATEGORY(lcPanel, "atlas.launcher.panel", QtInfoMsg)

namespace
{
// Space between the panel and the screen edge (or another panel's exclusive
// zone, which layer shell already keeps clear).
constexpr int kEdgeMargin = 12;
// Search opens this far down the screen, like Spotlight.
constexpr qreal kSearchTop = 0.22;
// A hidden panel gives its memory back this long after the hide.
constexpr int kTrimDelayMs = 60 * 1000;

QSize sizeProperty(const QQuickWindow *window, const char *name, QSize fallback)
{
    const QSize size = window->property(name).toSize();
    return size.isValid() && !size.isEmpty() ? size : fallback;
}
}

Panel::Panel(QQuickWindow *window, QObject *parent)
    : QObject(parent)
    , m_window(window)
{
    // Must come before the platform window exists: it makes the surface a
    // layer-shell one instead of an xdg toplevel. On X11 (tests under Xvfb)
    // there is no layer shell and the window stays an ordinary frameless one.
    if (QGuiApplication::platformName() == QLatin1String("wayland")) {
        m_layer = LayerShellQt::Window::get(window);
        m_layer->setScope(QStringLiteral("atlas-launcher"));
        m_layer->setLayer(LayerShellQt::Window::LayerTop);
        m_layer->setKeyboardInteractivity(LayerShellQt::Window::KeyboardInteractivityOnDemand);
        m_layer->setActivateOnShow(true);
        // Never reserve space; respect the dock's.
        m_layer->setExclusiveZone(0);
        // A screen going away dismisses the surface; keep the window so the
        // next open maps it on another screen.
        m_layer->setCloseOnDismissed(false);
        m_layer->setWantsToBeOnActiveScreen(true);
    }

    // Clicking anywhere else closes it, like Start and Spotlight. Not in
    // the first moments of a map: a mode switch unmaps and maps again, and
    // the old mapping's focus leave arrives after the new map.
    // A loss inside that moment is checked again once it has passed (a
    // single shot, only then), so a quick click away still closes it.
    connect(window, &QWindow::activeChanged, this, [this] {
        if (!m_window || !m_window->isVisible() || m_window->isActive() || !m_mappedAt.isValid()) {
            return;
        }
        const qint64 left = kFocusGraceMs - m_mappedAt.elapsed();
        if (left > 0) {
            QTimer::singleShot(left, this, [this] {
                if (m_window && m_window->isVisible() && !m_window->isActive()) {
                    qCDebug(lcPanel) << "focus lost, hiding";
                    hide();
                }
            });
            return;
        }
        qCDebug(lcPanel) << "focus lost, hiding";
        hide();
    });
    connect(window, &QWindow::visibleChanged, this, &Panel::shownChanged);
    m_trim.setSingleShot(true);
    m_trim.setInterval(kTrimDelayMs);
    connect(&m_trim, &QTimer::timeout, this, [this] {
        if (m_window && !m_window->isVisible()) {
            m_window->releaseResources();
            ::malloc_trim(0);
        }
    });
    connect(window, &QWindow::visibleChanged, this, [this](bool visible) {
        if (visible) {
            m_trim.stop();
        } else {
            m_trim.start();
        }
    });
    connect(window, &QWindow::widthChanged, this, &Panel::updateBlur);
    connect(window, &QWindow::heightChanged, this, &Panel::updateBlur);

    // Create the platform window now, at login, so opening only maps it.
    window->create();
}

QString Panel::mode() const
{
    return m_mode;
}

bool Panel::isShown() const
{
    return m_window && m_window->isVisible();
}

void Panel::setDockAnchor(const DockAnchor &anchor)
{
    m_dock = anchor;
}

void Panel::showStart(const DockAnchor &anchor, const QString &query)
{
    place(QStringLiteral("start"), anchor.screen.isEmpty() ? m_dock : anchor);
    map(query);
}

void Panel::showSearch(const QString &query)
{
    place(QStringLiteral("search"), {});
    map(query);
}

void Panel::toggleStart(const DockAnchor &anchor)
{
    if (isShown() && m_mode == QLatin1String("start")) {
        hide();
    } else {
        showStart(anchor);
    }
}

void Panel::toggleSearch()
{
    if (isShown() && m_mode == QLatin1String("search")) {
        hide();
    } else {
        showSearch();
    }
}

void Panel::hide()
{
    if (m_window && m_window->isVisible()) {
        m_window->hide();
    }
}

QScreen *Panel::screenNamed(const QString &name) const
{
    if (name.isEmpty()) {
        return nullptr;
    }
    const auto screens = QGuiApplication::screens();
    for (QScreen *screen : screens) {
        if (screen->name() == name) {
            return screen;
        }
    }
    return nullptr;
}

void Panel::place(const QString &mode, const DockAnchor &anchor)
{
    if (!m_window) {
        return;
    }
    // A mode change while shown (Meta while Search is open) re-places the
    // surface: unmap first, as layer-shell anchors apply on the next map.
    if (m_window->isVisible() && mode != m_mode) {
        m_window->hide();
    }
    if (m_mode != mode) {
        m_mode = mode;
        Q_EMIT modeChanged();
    }

    const bool start = mode == QLatin1String("start");
    const QSize size = start ? sizeProperty(m_window, "startSize", {640, 720}) : sizeProperty(m_window, "searchSize", {680, 480});
    m_window->resize(size);

    QScreen *screen = start ? screenNamed(anchor.screen) : nullptr;

    if (!m_layer) {
        // X11: plain placement on the primary (or named) screen.
        const QRect area = (screen ? screen : QGuiApplication::primaryScreen())->availableGeometry();
        int x = area.center().x() - size.width() / 2;
        int y = start ? area.bottom() - kEdgeMargin - size.height() : area.top() + int(area.height() * kSearchTop);
        if (start && screen && anchor.rect.isValid()) {
            // Centred on the screen, above the dock (as on Wayland, below).
            y = std::max(area.top() + kEdgeMargin, anchor.rect.top() - kEdgeMargin - size.height());
        }
        m_window->setPosition(x, y);
        return;
    }

    if (screen) {
        m_layer->setScreen(screen);
    } else {
        m_layer->setWantsToBeOnActiveScreen(true);
    }

    using A = LayerShellQt::Window;
    if (start) {
        if (screen && anchor.rect.isValid()) {
            // Centred on the dock's screen (the dock itself is centred; the
            // button is its first item, so centring on the button put the
            // panel off to the left), standing just above the dock, which the
            // button keeps shown while the panel is open.
            const QRect geo = screen->geometry();
            const int bottom = std::clamp(geo.bottom() + 1 - anchor.rect.top() + kEdgeMargin, kEdgeMargin, std::max(kEdgeMargin, geo.height() / 2));
            m_layer->setAnchors(A::Anchors(A::AnchorBottom));
            m_layer->setMargins({0, 0, 0, bottom});
        } else {
            // Bottom centre: anchoring one edge centres along it.
            m_layer->setAnchors(A::Anchors(A::AnchorBottom));
            m_layer->setMargins({0, 0, 0, kEdgeMargin});
        }
    } else {
        // The active screen's height is only known once mapped; use the
        // primary's as the estimate (all AtlasOS screens are landscape).
        const QScreen *estimate = QGuiApplication::primaryScreen();
        const int top = estimate ? int(estimate->availableGeometry().height() * kSearchTop) : 200;
        m_layer->setAnchors(A::Anchors(A::AnchorTop));
        m_layer->setMargins({0, top, 0, 0});
    }
}

void Panel::map(const QString &query)
{
    if (!m_window) {
        return;
    }
    qCDebug(lcPanel) << "show" << m_mode << m_window->size();
    Q_EMIT aboutToShow(m_mode, query);
    m_mappedAt.start();
    m_window->show();
    m_window->requestActivate();
    updateBlur();
}

void Panel::updateBlur()
{
    if (!m_window || !m_window->isVisible()) {
        return;
    }
    // The transparency switch (Telamon.Ui) turns blur off with the translucency.
    const bool blur = m_window->property("blurEnabled").toBool();
    if (!blur) {
        KWindowEffects::enableBlurBehind(m_window, false);
        return;
    }
    const qreal radius = m_window->property("cornerRadius").toReal();
    QPainterPath path;
    path.addRoundedRect(QRectF(QPointF(0, 0), QSizeF(m_window->size())), radius, radius);
    KWindowEffects::enableBlurBehind(m_window, true, QRegion(path.toFillPolygon().toPolygon()));
}
