// The checks of cpp/validate.h on the strings that reach the C++ side from
// outside (docs/SECURITY.md, "Starting programs"): ids, links, file URIs, web
// URLs and command paths. Run with ctest (-DTELAMON_LAUNCHER_BUILD_TESTS=ON).
#include "validate.h"

#include <QtTest>

#include <QFile>
#include <QTemporaryDir>

#include <sys/stat.h>

class ValidateTest : public QObject
{
    Q_OBJECT

private Q_SLOTS:
    void desktopId_data()
    {
        QTest::addColumn<QString>("id");
        QTest::addColumn<bool>("ok");
        QTest::newRow("plain") << QStringLiteral("org.kde.dolphin.desktop") << true;
        QTest::newRow("dashes and underscores") << QStringLiteral("a_b-c.desktop") << true;
        QTest::newRow("flatpak") << QStringLiteral("org.mozilla.firefox.desktop") << true;
        QTest::newRow("empty") << QString() << false;
        QTest::newRow("suffix only") << QStringLiteral(".desktop") << false;
        QTest::newRow("no suffix") << QStringLiteral("org.kde.dolphin") << false;
        QTest::newRow("absolute path") << QStringLiteral("/tmp/evil.desktop") << false;
        QTest::newRow("relative path") << QStringLiteral("../../tmp/evil.desktop") << false;
        QTest::newRow("slash inside") << QStringLiteral("a/b.desktop") << false;
        QTest::newRow("space") << QStringLiteral("a b.desktop") << false;
        QTest::newRow("newline") << QStringLiteral("a\nb.desktop") << false;
        QTest::newRow("trailing newline") << QStringLiteral("a.desktop\n") << false;
        QTest::newRow("NUL") << QStringLiteral("a") + QChar(0) + QStringLiteral("b.desktop") << false;
        QTest::newRow("unicode") << QStringLiteral("café.desktop") << false;
        QTest::newRow("bidi") << QStringLiteral("a‮.desktop") << false;
        QTest::newRow("scheme") << QStringLiteral("file:///tmp/x.desktop") << false;
        QTest::newRow("255 bytes") << QString(255 - 8, QLatin1Char('a')) + QStringLiteral(".desktop") << true;
        QTest::newRow("256 bytes") << QString(256 - 8, QLatin1Char('a')) + QStringLiteral(".desktop") << false;
        QTest::newRow("huge") << QString(10'000'000, QLatin1Char('a')) + QStringLiteral(".desktop") << false;
    }
    void desktopId()
    {
        QFETCH(QString, id);
        QFETCH(bool, ok);
        QCOMPARE(Validate::desktopId(id), ok);
    }

    void settingsLink_data()
    {
        QTest::addColumn<QString>("link");
        QTest::addColumn<bool>("ok");
        QTest::newRow("page") << QStringLiteral("bluetooth") << true;
        QTest::newRow("page and setting") << QStringLiteral("users/accounts") << true;
        QTest::newRow("dash inside") << QStringLiteral("a-b/c-d") << true;
        QTest::newRow("empty") << QString() << false;
        QTest::newRow("leading dash") << QStringLiteral("-x") << false;
        QTest::newRow("segment leading dash") << QStringLiteral("a/-x") << false;
        QTest::newRow("upper case") << QStringLiteral("Users") << false;
        QTest::newRow("empty segment") << QStringLiteral("a//b") << false;
        QTest::newRow("trailing slash") << QStringLiteral("a/") << false;
        QTest::newRow("dots") << QStringLiteral("../x") << false;
        QTest::newRow("space") << QStringLiteral("a b") << false;
        QTest::newRow("option") << QStringLiteral("--kcm=x") << false;
        QTest::newRow("129 bytes") << QString(129, QLatin1Char('a')) << false;
        QTest::newRow("128 bytes") << QString(128, QLatin1Char('a')) << true;
    }
    void settingsLink()
    {
        QFETCH(QString, link);
        QFETCH(bool, ok);
        QCOMPARE(Validate::settingsLink(link), ok);
    }

    void flatpakId_data()
    {
        QTest::addColumn<QString>("id");
        QTest::addColumn<bool>("ok");
        QTest::newRow("app") << QStringLiteral("org.mozilla.firefox") << true;
        QTest::newRow("two parts") << QStringLiteral("org.firefox") << false;
        QTest::newRow("leading dash") << QStringLiteral("--user.a.b") << false;
        QTest::newRow("empty part") << QStringLiteral("org..firefox") << false;
        QTest::newRow("space") << QStringLiteral("org.a b.c") << false;
        QTest::newRow("slash") << QStringLiteral("org.a/b.c") << false;
        QTest::newRow("newline") << QStringLiteral("org.a.b\n") << false;
        QTest::newRow("empty") << QString() << false;
    }
    void flatpakId()
    {
        QFETCH(QString, id);
        QFETCH(bool, ok);
        QCOMPARE(Validate::flatpakId(id), ok);
    }

    void fileUrl_data()
    {
        QTest::addColumn<QString>("uri");
        QTest::addColumn<bool>("ok");
        QTest::newRow("file") << QStringLiteral("file:///home/u/a.txt") << true;
        QTest::newRow("localhost") << QStringLiteral("file://localhost/home/u/a.txt") << true;
        QTest::newRow("encoded space") << QStringLiteral("file:///home/u/a%20b.txt") << true;
        QTest::newRow("empty") << QString() << false;
        QTest::newRow("http") << QStringLiteral("http://example.com/a") << false;
        QTest::newRow("https") << QStringLiteral("https://example.com/a") << false;
        QTest::newRow("other host") << QStringLiteral("file://evil.example/share/a") << false;
        QTest::newRow("smb") << QStringLiteral("smb://host/share/a") << false;
        QTest::newRow("query") << QStringLiteral("file:///a?x=1") << false;
        QTest::newRow("fragment") << QStringLiteral("file:///a#x") << false;
        QTest::newRow("user") << QStringLiteral("file://user@localhost/a") << false;
        QTest::newRow("port") << QStringLiteral("file://localhost:80/a") << false;
        QTest::newRow("relative") << QStringLiteral("a/b.txt") << false;
        QTest::newRow("encoded NUL") << QStringLiteral("file:///a%00b") << false;
        QTest::newRow("dotdot") << QStringLiteral("file:///a/../etc/passwd") << false;
        QTest::newRow("encoded dotdot") << QStringLiteral("file:///a/%2e%2e/etc/passwd") << false;
        QTest::newRow("dot") << QStringLiteral("file:///a/./b") << false;
        QTest::newRow("newline") << QStringLiteral("file:///a\nb") << false;
        QTest::newRow("too long") << QStringLiteral("file:///") + QString(13000, QLatin1Char('a')) << false;
    }
    void fileUrl()
    {
        QFETCH(QString, uri);
        QFETCH(bool, ok);
        const QUrl url = Validate::fileUrl(uri);
        QCOMPARE(url.isValid() && !url.isEmpty(), ok);
        if (ok) {
            QVERIFY(url.isLocalFile());
            QVERIFY(url.host().isEmpty());
        }
    }

    void longestUrls()
    {
        // The longest search the core builds: 1,024 bytes of CJK text (341
        // characters of three bytes and one ASCII byte), each non-ASCII byte "%XX".
        QString query;
        for (int i = 0; i < 341; ++i) {
            query += QStringLiteral("%E4%B8%AD");
        }
        query += QLatin1Char('A'); // 1,023 + 1 bytes: the 1,024 budget
        const QString longest = QStringLiteral("https://www.startpage.com/do/search?q=") + query;
        QVERIFY(longest.size() > 2048);
        QVERIFY(longest.size() <= Validate::kMaxWebUrlLength);
        QVERIFY(Validate::webUrl(longest).isValid());
        // The very longest by the formula (a 38-character prefix and 3,072).
        QString maxed = QStringLiteral("https://www.startpage.com/do/search?q=") + QString(3 * 1024, QLatin1Char('A'));
        QVERIFY(maxed.size() <= Validate::kMaxWebUrlLength);
        QVERIFY(Validate::webUrl(maxed).isValid());
        // One past the bound is refused.
        QVERIFY(!Validate::webUrl(QStringLiteral("https://x.example/?q=") + QString(Validate::kMaxWebUrlLength, QLatin1Char('A'))).isValid());
        // A file URI of a path at the core's 4,096-byte cap, all non-ASCII.
        QString file = QStringLiteral("file:///");
        for (int i = 0; i < 4096 / 3; ++i) {
            file += QStringLiteral("%E4%B8%AD");
        }
        QVERIFY(file.size() > 2048);
        QVERIFY(file.size() <= Validate::kMaxFileUrlLength);
        QVERIFY(Validate::fileUrl(file).isValid());
        QVERIFY(!Validate::fileUrl(QStringLiteral("file:///") + QString(Validate::kMaxFileUrlLength, QLatin1Char('a'))).isValid());
    }

    void webUrl_data()
    {
        QTest::addColumn<QString>("url");
        QTest::addColumn<bool>("ok");
        QTest::newRow("search") << QStringLiteral("https://duckduckgo.com/?q=a%20b") << true;
        QTest::newRow("http") << QStringLiteral("http://duckduckgo.com/?q=a") << false;
        QTest::newRow("file") << QStringLiteral("file:///etc/passwd") << false;
        QTest::newRow("javascript") << QStringLiteral("javascript:alert(1)") << false;
        QTest::newRow("data") << QStringLiteral("data:text/html,x") << false;
        QTest::newRow("no host") << QStringLiteral("https:///x") << false;
        QTest::newRow("user") << QStringLiteral("https://user:pw@example.com/") << false;
        QTest::newRow("port") << QStringLiteral("https://example.com:8443/") << false;
        QTest::newRow("space") << QStringLiteral("https://example.com/a b") << false;
        QTest::newRow("newline") << QStringLiteral("https://example.com/a\nb") << false;
        QTest::newRow("empty") << QString() << false;
        QTest::newRow("too long") << QStringLiteral("https://example.com/") + QString(3200, QLatin1Char('a')) << false;
    }
    void webUrl()
    {
        QFETCH(QString, url);
        QFETCH(bool, ok);
        const QUrl checked = Validate::webUrl(url);
        QCOMPARE(checked.isValid() && !checked.isEmpty(), ok);
    }

    void displayText_data()
    {
        QTest::addColumn<QString>("in");
        QTest::addColumn<QString>("out");
        QTest::newRow("plain") << QStringLiteral("Ada Lovelace") << QStringLiteral("Ada Lovelace");
        QTest::newRow("spaces") << QStringLiteral("  Ada \t Lovelace\n") << QStringLiteral("Ada Lovelace");
        QTest::newRow("bidi override") << QStringLiteral("Ada\u202eecalevoL") << QStringLiteral("Ada ecalevoL");
        QTest::newRow("isolates") << QStringLiteral("a\u2066b\u2069c") << QStringLiteral("a b c");
        QTest::newRow("zero width") << QStringLiteral("a\u200bb\u200dc\ufeffd") << QStringLiteral("a b c d");
        QTest::newRow("NUL and escape") << QStringLiteral("a") + QChar(0) + QStringLiteral("b\x1b[31mc") << QStringLiteral("a b [31mc");
        QTest::newRow("line separator") << QStringLiteral("a\u2028b\u2029c") << QStringLiteral("a b c");
        QTest::newRow("soft hyphen") << QStringLiteral("a\u00adb") << QStringLiteral("a b");
        QTest::newRow("unpaired surrogate") << QStringLiteral("a") + QChar(0xD800) + QStringLiteral("b") << QStringLiteral("a b");
        QTest::newRow("emoji kept") << QStringLiteral("Zoe \U0001F600") << QStringLiteral("Zoe \U0001F600");
        QTest::newRow("only invisible") << QStringLiteral("\u202e\u200b") << QString();
        QTest::newRow("empty") << QString() << QString();
    }
    void displayText()
    {
        QFETCH(QString, in);
        QFETCH(QString, out);
        QCOMPARE(Validate::displayText(in, 256), out);
    }
    void displayTextCaps()
    {
        QCOMPARE(Validate::displayText(QString(1000, QLatin1Char('x')), 256).size(), 256);
        QCOMPARE(Validate::displayText(QStringLiteral("ab cd ef"), 5), QStringLiteral("ab cd"));
        QCOMPARE(Validate::displayText(QString(10'000'000, QLatin1Char(' ')), 256), QString());
    }

    void plainSmallFile()
    {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        const QString base = dir.path();
        // Absent: fine, the reader will not find it.
        QVERIFY(Validate::plainSmallFile(base + QStringLiteral("/absent"), 100));
        // A small regular file, and a link to it.
        QFile small(base + QStringLiteral("/small"));
        QVERIFY(small.open(QIODevice::WriteOnly));
        small.write("abc");
        small.close();
        QVERIFY(Validate::plainSmallFile(small.fileName(), 100));
        QVERIFY(QFile::link(small.fileName(), base + QStringLiteral("/link")));
        QVERIFY(Validate::plainSmallFile(base + QStringLiteral("/link"), 100));
        // Over the cap, exactly at it.
        QVERIFY(Validate::plainSmallFile(small.fileName(), 3));
        QVERIFY(!Validate::plainSmallFile(small.fileName(), 2));
        QFile big(base + QStringLiteral("/big"));
        QVERIFY(big.open(QIODevice::WriteOnly));
        QVERIFY(big.resize(10'000'000));
        big.close();
        QVERIFY(!Validate::plainSmallFile(big.fileName(), 64 * 1024));
        // A pipe, and a link to a pipe: refused without ever being opened.
        const QByteArray fifo = QFile::encodeName(base + QStringLiteral("/pipe"));
        QCOMPARE(::mkfifo(fifo.constData(), 0600), 0);
        QVERIFY(!Validate::plainSmallFile(base + QStringLiteral("/pipe"), 100));
        QVERIFY(QFile::link(base + QStringLiteral("/pipe"), base + QStringLiteral("/pipelink")));
        QVERIFY(!Validate::plainSmallFile(base + QStringLiteral("/pipelink"), 100));
        // A folder, and a device.
        QVERIFY(!Validate::plainSmallFile(base, 100));
        QVERIFY(!Validate::plainSmallFile(QStringLiteral("/dev/zero"), 100));
    }

    void commandPath_data()
    {
        QTest::addColumn<QString>("path");
        QTest::addColumn<bool>("ok");
        QTest::newRow("absolute") << QStringLiteral("/usr/bin/ls") << true;
        QTest::newRow("with space") << QStringLiteral("/opt/my tools/run") << true;
        QTest::newRow("qt resource") << QStringLiteral(":/x") << false;
        QTest::newRow("qt resource tool") << QStringLiteral(":/bin/ls") << false;
        QTest::newRow("double slash") << QStringLiteral("//host/share/x") << false;
        QTest::newRow("triple slash") << QStringLiteral("///usr/bin/ls") << false;
        QTest::newRow("dotdot") << QStringLiteral("/usr/bin/../../bin/sh") << false;
        QTest::newRow("trailing dotdot") << QStringLiteral("/usr/bin/..") << false;
        QTest::newRow("a name with two dots") << QStringLiteral("/usr/bin/a..b") << true;
        QTest::newRow("a dot segment") << QStringLiteral("/usr/./bin/ls") << true;
        QTest::newRow("single dot dir") << QStringLiteral("/opt/x.y/ls") << true;
        QTest::newRow("bare name") << QStringLiteral("ls") << false;
        QTest::newRow("relative") << QStringLiteral("./ls") << false;
        QTest::newRow("home") << QStringLiteral("~/bin/ls") << false;
        QTest::newRow("empty") << QString() << false;
        QTest::newRow("NUL") << QStringLiteral("/usr/bin/l") + QChar(0) + QStringLiteral("s") << false;
        QTest::newRow("too long") << QStringLiteral("/") + QString(5000, QLatin1Char('a')) << false;
    }
    void commandPath()
    {
        QFETCH(QString, path);
        QFETCH(bool, ok);
        QCOMPARE(Validate::commandPath(path, 4096), ok);
    }
};

QTEST_APPLESS_MAIN(ValidateTest)
#include "validate_test.moc"
