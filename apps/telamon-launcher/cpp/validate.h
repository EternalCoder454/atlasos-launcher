// The checks on text that reaches the C++ side from outside, in one header with
// no dependency beyond QtCore, so tests/validate_test.cpp covers exactly what
// the app runs (docs/SECURITY.md, "Starting programs"). The Rust core checks
// the same things where they enter; these are the second line, in front of the
// calls that start something.
#pragma once

#include <QChar>
#include <QDir>
#include <QFileInfo>
#include <QRegularExpression>
#include <QString>
#include <QStringList>
#include <QUrl>

namespace Validate
{
inline constexpr int kMaxIdLength = 255;
inline constexpr int kMaxUrlLength = 2048;

// A desktop file id: [A-Za-z0-9._-]{1,255} ending in ".desktop". Nothing else
// is handed to KService::serviceByStorageId: it takes an absolute path for a
// storage id and then loads any desktop file there, so "/tmp/x.desktop" would
// start an app the catalogue never listed.
inline bool desktopId(const QString &id)
{
    static const QString suffix = QStringLiteral(".desktop");
    if (id.size() <= suffix.size() || id.size() > kMaxIdLength || !id.endsWith(suffix)) {
        return false;
    }
    for (const QChar c : id) {
        const ushort u = c.unicode();
        const bool ok = (u >= 'a' && u <= 'z') || (u >= 'A' && u <= 'Z') || (u >= '0' && u <= '9') || u == '.' || u == '_' || u == '-';
        if (!ok) {
            return false;
        }
    }
    return true;
}

// A Settings deep link: [a-z0-9][a-z0-9-]*(/[a-z0-9][a-z0-9-]*)*, at most 128
// bytes (the same rule as the Settings index's links in the Rust core).
inline bool settingsLink(const QString &link)
{
    if (link.isEmpty() || link.size() > 128) {
        return false;
    }
    const QStringList parts = link.split(QLatin1Char('/'));
    for (const QString &part : parts) {
        if (part.isEmpty()) {
            return false;
        }
        for (int i = 0; i < part.size(); ++i) {
            const ushort u = part.at(i).unicode();
            const bool lower = (u >= 'a' && u <= 'z') || (u >= '0' && u <= '9');
            if (!(lower || (u == '-' && i > 0))) {
                return false;
            }
        }
    }
    return true;
}

// A Flatpak id: reverse-DNS, 3+ parts, no leading dash (it becomes an argument
// of telamon-store).
inline bool flatpakId(const QString &id)
{
    // \A...\z: a "$" would let a trailing newline through.
    static const QRegularExpression pattern(QStringLiteral("\\A[A-Za-z0-9][A-Za-z0-9._-]*\\z"));
    if (id.isEmpty() || id.size() > kMaxIdLength || !pattern.match(id).hasMatch()) {
        return false;
    }
    const QStringList parts = id.split(QLatin1Char('.'));
    if (parts.size() < 3) {
        return false;
    }
    for (const QString &part : parts) {
        if (part.isEmpty()) {
            return false;
        }
    }
    return true;
}

// A file URI from outside, checked: a local file, no host (or localhost), no
// query, no fragment, no user, no port, no "." or ".." segment. An invalid QUrl when it fails; else a
// clean file:/// URL.
inline QUrl fileUrl(const QString &uri)
{
    if (uri.isEmpty() || uri.size() > kMaxUrlLength) {
        return {};
    }
    const QUrl url(uri, QUrl::StrictMode);
    const bool localHost = url.host().isEmpty() || url.host() == QLatin1String("localhost");
    if (!url.isValid() || !url.isLocalFile() || !localHost || url.hasQuery() || url.hasFragment() || !url.userInfo().isEmpty() || url.port() != -1) {
        return {};
    }
    const QString path = url.path(QUrl::FullyDecoded);
    if (!path.startsWith(QLatin1Char('/')) || path.contains(QChar(0))) {
        return {};
    }
    // The Rust core only passes clean paths on; a "." or ".." segment is not one.
    const QStringList segments = path.split(QLatin1Char('/'));
    for (const QString &segment : segments) {
        if (segment == QLatin1String(".") || segment == QLatin1String("..")) {
            return {};
        }
    }
    return QUrl::fromLocalFile(path);
}

// An https URL for the browser: a host, no user or port, no control character.
// An invalid QUrl when it fails.
inline QUrl webUrl(const QString &text)
{
    if (text.isEmpty() || text.size() > kMaxUrlLength) {
        return {};
    }
    const QUrl url(text, QUrl::StrictMode);
    if (!url.isValid() || url.scheme() != QLatin1String("https") || url.host().isEmpty() || !url.userInfo().isEmpty() || url.port() != -1) {
        return {};
    }
    return url;
}

// Text from a system service made safe to show (the user's real name from
// AccountsService): control, format (bidi, zero-width, joiners, soft hyphen,
// tags), separator-line/paragraph, surrogate and private-use characters are
// replaced by a space, runs of white space made one, the ends trimmed, and at
// most `maxChars` characters kept. The Rust core's `clean_display` does more
// (it also knows the Hangul fillers, the Braille blank and the variation
// selectors); this covers what Qt's character categories can tell.
inline QString displayText(const QString &text, int maxChars)
{
    QString out;
    out.reserve(qMin<qsizetype>(text.size(), maxChars));
    bool pendingSpace = false;
    int count = 0;
    for (qsizetype i = 0; i < text.size(); ++i) {
        // One code point: a surrogate pair is one character, a lone surrogate
        // is not a character at all.
        char32_t cp = text.at(i).unicode();
        qsizetype units = 1;
        if (QChar::isHighSurrogate(cp) && i + 1 < text.size() && text.at(i + 1).isLowSurrogate()) {
            cp = QChar::surrogateToUcs4(text.at(i), text.at(i + 1));
            units = 2;
            ++i;
        }
        bool space = QChar::isSpace(cp);
        switch (QChar::category(cp)) {
        case QChar::Other_Control:
        case QChar::Other_Format:
        case QChar::Other_Surrogate:
        case QChar::Other_PrivateUse:
        case QChar::Other_NotAssigned:
        case QChar::Separator_Line:
        case QChar::Separator_Paragraph:
            space = true;
            break;
        default:
            break;
        }
        if (space) {
            pendingSpace = !out.isEmpty();
            continue;
        }
        if (count + (pendingSpace ? 2 : 1) > maxChars) {
            break;
        }
        if (pendingSpace) {
            out.append(QLatin1Char(' '));
            ++count;
            pendingSpace = false;
        }
        out.append(text.mid(i + 1 - units, units));
        ++count;
    }
    return out;
}

// Whether `path` may be handed to a reader that opens it as it is: it does not
// exist (yet), or it is a regular file (links followed) of at most `maxBytes`.
// A pipe in its place would make the open wait for a writer for good (measured:
// the GUI thread of the launcher stood in wait_for_partner and the launcher
// never answered again); a huge file would be read into memory whole. One
// stat, no open.
inline bool plainSmallFile(const QString &path, qint64 maxBytes)
{
    const QFileInfo info(path);
    return !info.exists() || (info.isFile() && info.size() <= maxBytes);
}

// The program of a typed command: the absolute path the PATH scan found.
// Never a bare name (that would be looked up on a PATH again, at launch).
inline bool commandPath(const QString &executable, int maxLength)
{
    return !executable.isEmpty() && executable.size() <= maxLength && !executable.contains(QChar(0)) && QDir::isAbsolutePath(executable);
}
}
