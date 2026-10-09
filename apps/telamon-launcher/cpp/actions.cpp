#include "actions.h"

#include "executor.h"
#include "validate.h"

#include <KRecentDocument>
#include <KUser>

#include <QDBusMessage>
#include <QDBusObjectPath>
#include <QDBusPendingCallWatcher>
#include <QDBusPendingReply>
#include <QDBusVariant>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QImageReader>
#include <QLoggingCategory>
#include <QMetaObject>
#include <QRegularExpression>
#include <QStandardPaths>
#include <QThreadPool>
#include <QUrl>
#include <QVariantMap>

#include <algorithm>

#include <sys/stat.h>
#include <unistd.h>

Q_LOGGING_CATEGORY(lcActions, "telamon.launcher.actions", QtInfoMsg)

namespace
{
constexpr int kMaxTextLength = 1 << 20;
constexpr qint64 kMaxIconBytes = 16 << 20;
// The user picture is shown small; a header that claims more is not decoded.
constexpr int kMaxIconSide = 4096;

// The picture is a path AccountsService stores for the user: it must be an
// absolute regular file (not a link, so nothing can point it elsewhere) that
// the user or root owns, of sane size and dimensions.
bool trustedIconFile(const QString &path)
{
    if (path.isEmpty() || path.size() > 4096 || !path.startsWith(QLatin1Char('/')) || path.contains(QChar(0))) {
        return false;
    }
    struct stat st {};
    if (::lstat(QFile::encodeName(path).constData(), &st) != 0) {
        return false;
    }
    if (!S_ISREG(st.st_mode) || (st.st_uid != ::getuid() && st.st_uid != 0)) {
        return false;
    }
    if (st.st_size <= 0 || st.st_size > kMaxIconBytes) {
        return false;
    }
    // Header only: no decoding.
    const QSize size = QImageReader(path).size();
    return size.isValid() && size.width() <= kMaxIconSide && size.height() <= kMaxIconSide;
}

bool capabilityAllowed(const QString &answer)
{
    // "challenge" means polkit would ask, which the user can answer.
    return answer == QLatin1String("yes") || answer == QLatin1String("challenge");
}
}

Actions::Actions(Executor *executor, QObject *backend, QObject *parent)
    : QObject(parent)
    , m_executor(executor)
    , m_backend(backend)
{
    connect(m_executor, &Executor::failed, this, &Actions::failed);
}

QString Actions::userName() const
{
    // Read when first asked, not at startup: the environment first, then the
    // password database (which can be slow behind NSS).
    if (m_userName.isEmpty() && !m_userNameRead) {
        m_userNameRead = true;
        m_userName = Validate::displayText(qEnvironmentVariable("USER"), 256);
        if (m_userName.isEmpty()) {
            m_userName = KUser().loginName();
        }
    }
    return m_userName;
}

void Actions::load()
{
    loadCapability("CanSuspend", &Actions::m_canSuspend);
    loadCapability("CanHibernate", &Actions::m_canHibernate);
    loadSeat();
    loadUser();
}

void Actions::answered()
{
    Q_EMIT changed();
    if (m_waiting > 0 && --m_waiting == 0) {
        Q_EMIT capabilitiesReady();
    }
}

void Actions::loadCapability(const char *method, bool Actions::*member)
{
    auto message = QDBusMessage::createMethodCall(QStringLiteral("org.freedesktop.login1"),
                                                  QStringLiteral("/org/freedesktop/login1"),
                                                  QStringLiteral("org.freedesktop.login1.Manager"),
                                                  QString::fromLatin1(method));
    auto *watcher = m_executor->dbusCall(message, true);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [this, member](QDBusPendingCallWatcher *w) {
        w->deleteLater();
        const QDBusPendingReply<QString> reply = *w;
        this->*member = reply.isValid() && capabilityAllowed(reply.value());
        answered();
    });
}

void Actions::loadSeat()
{
    // Switching user needs a seat that holds more than one session.
    auto message = QDBusMessage::createMethodCall(QStringLiteral("org.freedesktop.login1"),
                                                  QStringLiteral("/org/freedesktop/login1/seat/self"),
                                                  QStringLiteral("org.freedesktop.DBus.Properties"),
                                                  QStringLiteral("Get"));
    message << QStringLiteral("org.freedesktop.login1.Seat") << QStringLiteral("CanMultiSession");
    auto *watcher = m_executor->dbusCall(message, true);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *w) {
        w->deleteLater();
        const QDBusPendingReply<QDBusVariant> reply = *w;
        m_canSwitchUser = reply.isValid() && reply.value().variant().toBool();
        answered();
    });
}

void Actions::loadUser()
{
    auto message = QDBusMessage::createMethodCall(QStringLiteral("org.freedesktop.Accounts"),
                                                  QStringLiteral("/org/freedesktop/Accounts"),
                                                  QStringLiteral("org.freedesktop.Accounts"),
                                                  QStringLiteral("FindUserById"));
    message << qlonglong(::getuid());
    auto *find = m_executor->dbusCall(message, true);
    connect(find, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *w) {
        w->deleteLater();
        const QDBusPendingReply<QDBusObjectPath> reply = *w;
        if (!reply.isValid()) {
            qCInfo(lcActions) << "AccountsService has no entry for the user";
            return;
        }
        auto props = QDBusMessage::createMethodCall(QStringLiteral("org.freedesktop.Accounts"),
                                                    reply.value().path(),
                                                    QStringLiteral("org.freedesktop.DBus.Properties"),
                                                    QStringLiteral("GetAll"));
        props << QStringLiteral("org.freedesktop.Accounts.User");
        auto *all = m_executor->dbusCall(props, true);
        connect(all, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *p) {
            p->deleteLater();
            const QDBusPendingReply<QVariantMap> values = *p;
            if (!values.isValid()) {
                return;
            }
            // From a system service, but another user with admin rights sets it:
            // shown as plain text and cleaned of control and bidi characters.
            const QString realName = Validate::displayText(values.value().value(QStringLiteral("RealName")).toString(), 256);
            if (!realName.isEmpty()) {
                m_userName = realName;
            }
            // The picture is a path someone else wrote: it must be an
            // absolute, readable, regular file of sane size.
            const QString icon = values.value().value(QStringLiteral("IconFile")).toString();
            if (trustedIconFile(icon)) {
                m_userIcon = icon;
            } else if (!icon.isEmpty()) {
                qCInfo(lcActions) << "the user picture was not used (not a plain file of the user or root, or too large)";
            }
            Q_EMIT changed();
        });
    });
}

void Actions::power(const QString &name)
{
    m_executor->session(name, false);
}

void Actions::openContainingFolder(const QString &uri)
{
    m_executor->showInFolder(uri);
}

void Actions::copyText(const QString &text)
{
    if (text.size() > kMaxTextLength) {
        Q_EMIT failed(QStringLiteral("BadArgument"));
        return;
    }
    m_executor->copy(text);
}

void Actions::removeRecent(const QString &uri)
{
    const QUrl url = Executor::checkedFileUrl(uri);
    if (!url.isValid()) {
        Q_EMIT failed(QStringLiteral("BadArgument"));
        return;
    }
    // KRecentDocument walks the recent-documents folder: off the GUI thread.
    // The answer goes back through this object, which the queued call
    // drops if it is gone.
    QThreadPool::globalInstance()->start([this, url] {
        KRecentDocument::removeFile(url);
        QMetaObject::invokeMethod(
            this,
            [this] {
                QMetaObject::invokeMethod(m_backend, "refresh", Qt::QueuedConnection);
            },
            Qt::QueuedConnection);
    });
}

void Actions::openAppSettings(const QString &desktopId)
{
    if (!Validate::desktopId(desktopId)) {
        Q_EMIT failed(QStringLiteral("BadArgument"));
        return;
    }
    m_executor->activateSettings(QStringLiteral("open-app"), {desktopId});
}

void Actions::openUserSettings()
{
    m_executor->activateSettings(QStringLiteral("open"), {QStringLiteral("users")});
}

void Actions::openSettingsApp()
{
    m_executor->activateSettings({}, {});
}

void Actions::openFiles()
{
    // The user's home folder, in whatever file manager they prefer.
    m_executor->openLocalFile(QUrl::fromLocalFile(QDir::homePath()).toString());
}

void Actions::uninstall(const QString &flatpakId)
{
    if (!Validate::flatpakId(flatpakId)) {
        Q_EMIT failed(QStringLiteral("BadArgument"));
        return;
    }
    // An absolute path: never whatever comes first in the caller's PATH.
    // Telamon Store, else the one of the name before the rename.
    const QStringList dirs{QStringLiteral("/usr/bin"), QStringLiteral("/usr/local/bin")};
    QString store = QStandardPaths::findExecutable(QStringLiteral("telamon-store"), dirs);
    if (store.isEmpty()) {
        store = QStandardPaths::findExecutable(QStringLiteral("atlas-store"), dirs);
    }
    if (store.isEmpty()) {
        qCWarning(lcActions) << "telamon-store is not installed";
        Q_EMIT failed(QStringLiteral("StoreMissing"));
        return;
    }
    m_executor->startCommand(store, {QStringLiteral("--remove"), flatpakId});
}

void Actions::launchAction(const QString &desktopId, const QString &actionId)
{
    m_executor->launchApplication(desktopId, actionId);
}

void Actions::editApplications()
{
    m_executor->launchApplication(QStringLiteral("org.kde.kmenuedit.desktop"), {});
}
