// Starts the launcher: one resident process per session that keeps its panel
// created and hidden, so Meta only has to show it (docs/DESIGN.md, "Form").
// Launch arguments of a second start (`atlas-launcher --search`) go to the
// first through KDBusService.
#include <atlas/app.h>

#include <KDBusService>

#include <QApplication>
#include <QCommandLineParser>
#include <QQmlApplicationEngine>
#include <QQuickWindow>
#include <QSGRendererInterface>

// Defined in src/lib.rs.
extern "C" void *atlas_backend_new();

int main(int argc, char *argv[])
{
    atlas_app_init();
    // Drawn on the CPU like the other Atlas apps unless QT_QUICK_BACKEND says
    // otherwise; the P phase measures software against the GPU.
    if (qEnvironmentVariableIsEmpty("QT_QUICK_BACKEND")) {
        QQuickWindow::setGraphicsApi(QSGRendererInterface::Software);
    }
    // The launcher stays running with its panel hidden.
    QApplication::setQuitOnLastWindowClosed(false);
    QApplication app(argc, argv);
    atlas_app_ready();

    QCommandLineParser parser;
    parser.setApplicationDescription(QStringLiteral("The Start menu and search of AtlasOS."));
    parser.addHelpOption();
    parser.addVersionOption();
    parser.process(app);

    KDBusService service(KDBusService::Unique);

    auto *backend = static_cast<QObject *>(atlas_backend_new());
    QQmlApplicationEngine engine;
    engine.setInitialProperties({{QStringLiteral("backend"), QVariant::fromValue(backend)}});
    engine.loadFromModule("net.eterneon.atlas.launcher", "Panel");
    if (engine.rootObjects().isEmpty()) {
        return 1;
    }
    const int code = app.exec();
    delete backend;
    return code;
}
