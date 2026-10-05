#include "dbus.h"

#include "panel.h"

#include <QDBusArgument>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QLoggingCategory>
#include <QRect>

#include <optional>
#include <utility>

Q_LOGGING_CATEGORY(lcDBus, "atlas.launcher.dbus", QtInfoMsg)

namespace
{
constexpr auto kPath = "/net/eterneon/atlas/launcher";
constexpr auto kInterface = "net.eterneon.atlas.Launcher1";
// Bounds for untrusted input (docs/DESIGN.md, "Trust").
constexpr int kMaxScreenName = 64;
constexpr int kMaxQuery = 256;
constexpr int kMaxCoordinate = 1 << 16;
constexpr int kMaxPins = 64;
constexpr int kClearHistoryTimeoutMs = 5000;

// `anchor` is (iiii) as DESIGN.md says; an array of four ints (ai or av) is
// accepted too, for callers (QML) that cannot build a struct.
std::optional<QRect> parseRect(const QVariant &value)
{
    QList<int> v;
    if (value.canConvert<QDBusArgument>()) {
        const auto arg = value.value<QDBusArgument>();
        if (arg.currentSignature() == QLatin1String("(iiii)")) {
            int x = 0, y = 0, w = 0, h = 0;
            arg.beginStructure();
            arg >> x >> y >> w >> h;
            arg.endStructure();
            v = {x, y, w, h};
        } else if (arg.currentSignature() == QLatin1String("ai")) {
            arg >> v;
        } else if (arg.currentSignature() == QLatin1String("av")) {
            // What Plasma's QML D-Bus module sends for a JS array.
            QVariantList items;
            arg >> items;
            return parseRect(QVariant(items));
        }
    } else if (value.metaType().id() == QMetaType::QVariantList) {
        const QVariantList items = value.toList();
        if (items.size() != 4) {
            return std::nullopt;
        }
        for (const QVariant &item : items) {
            bool ok = false;
            v.append(item.toInt(&ok));
            if (!ok) {
                return std::nullopt;
            }
        }
    }
    if (v.size() != 4) {
        return std::nullopt;
    }
    for (int n : std::as_const(v)) {
        if (n < -kMaxCoordinate || n > kMaxCoordinate) {
            return std::nullopt;
        }
    }
    if (v[2] <= 0 || v[3] <= 0) {
        return std::nullopt;
    }
    return QRect(v[0], v[1], v[2], v[3]);
}

Panel::DockAnchor parseAnchor(const QVariantMap &options)
{
    Panel::DockAnchor anchor;
    const QString screen = options.value(QStringLiteral("screen")).toString();
    const auto rect = parseRect(options.value(QStringLiteral("anchor")));
    if (screen.isEmpty() || screen.size() > kMaxScreenName || !rect) {
        return {};
    }
    anchor.screen = screen;
    anchor.rect = *rect;
    return anchor;
}
}

LauncherAdaptor::LauncherAdaptor(QObject *parent, Panel *panel)
    : QDBusAbstractAdaptor(parent)
    , m_panel(panel)
{
    connect(panel, &Panel::shownChanged, this, &LauncherAdaptor::emitVisibleChanged);
    m_clearTimeout.setSingleShot(true);
    m_clearTimeout.setInterval(kClearHistoryTimeoutMs);
    connect(&m_clearTimeout, &QTimer::timeout, this, [this] {
        qCWarning(lcDBus) << "ClearHistory: the backend did not answer in time";
        replyToClearHistory(false, QStringLiteral("The history was not cleared in time."));
    });
}

bool LauncherAdaptor::visible() const
{
    return m_panel->isShown();
}

void LauncherAdaptor::ToggleStart()
{
    m_panel->toggleStart();
}

void LauncherAdaptor::ToggleStart(const QVariantMap &options)
{
    m_panel->toggleStart(parseAnchor(options));
}

void LauncherAdaptor::ToggleSearch()
{
    m_panel->toggleSearch();
}

void LauncherAdaptor::Show(const QString &mode, const QString &query, const QVariantMap &platformData)
{
    Q_UNUSED(platformData)
    // The query is only typed into the field; nothing runs until the user
    // presses Enter in the panel.
    const QString text = query.left(kMaxQuery);
    if (mode == QLatin1String("start")) {
        m_panel->showStart({}, text);
    } else if (mode == QLatin1String("search")) {
        m_panel->showSearch(text);
    } else if (calledFromDBus()) {
        sendErrorReply(QDBusError::InvalidArgs, QStringLiteral("mode must be \"start\" or \"search\""));
    }
}

void LauncherAdaptor::Hide()
{
    m_panel->hide();
}

void LauncherAdaptor::SetDockAnchor(const QVariantMap &options)
{
    const auto anchor = parseAnchor(options);
    if (anchor.screen.isEmpty()) {
        qCWarning(lcDBus) << "SetDockAnchor: ignored an anchor without a valid screen and rect";
        return;
    }
    m_panel->setDockAnchor(anchor);
}

void LauncherAdaptor::ImportPins(const QStringList &ids)
{
    // Validated and resolved by the backend; only the count is bounded here.
    Q_EMIT importPinsRequested(ids.mid(0, kMaxPins));
}

void LauncherAdaptor::ClearHistory()
{
    if (!calledFromDBus()) {
        Q_EMIT clearHistoryRequested();
        return;
    }
    // The reply goes out when the backend says it is done.
    setDelayedReply(true);
    const bool first = m_clearPending.isEmpty();
    m_clearPending.append(message());
    if (first) {
        m_clearTimeout.start();
        Q_EMIT clearHistoryRequested();
    }
}

void LauncherAdaptor::onHistoryCleared(bool ok)
{
    replyToClearHistory(ok, QStringLiteral("The history could not be cleared."));
}

void LauncherAdaptor::replyToClearHistory(bool ok, const QString &error)
{
    m_clearTimeout.stop();
    const QList<QDBusMessage> pending = std::exchange(m_clearPending, {});
    QDBusConnection bus = QDBusConnection::sessionBus();
    for (const QDBusMessage &call : pending) {
        bus.send(ok ? call.createReply() : call.createErrorReply(QDBusError::Failed, error));
    }
}

void LauncherAdaptor::emitVisibleChanged()
{
    QDBusMessage signal = QDBusMessage::createSignal(QString::fromLatin1(kPath),
                                                     QStringLiteral("org.freedesktop.DBus.Properties"),
                                                     QStringLiteral("PropertiesChanged"));
    signal << QString::fromLatin1(kInterface) << QVariantMap{{QStringLiteral("Visible"), m_panel->isShown()}} << QStringList();
    QDBusConnection::sessionBus().send(signal);
}
