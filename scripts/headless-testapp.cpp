// A window that stands in for an app in the headless dock test
// (scripts/headless-dock.sh): its Wayland app id is the argument, as a real
// app's is its desktop file name, and its title the second argument.
#include <QGuiApplication>
#include <QQuickView>
#include <QUrl>

int main(int argc, char **argv)
{
    QGuiApplication app(argc, argv);
    if (argc < 3) {
        return 2;
    }
    QGuiApplication::setDesktopFileName(QString::fromLocal8Bit(argv[1]));
    QQuickView view;
    view.setTitle(QString::fromLocal8Bit(argv[2]));
    view.resize(360, 240);
    view.setColor(Qt::darkCyan);
    view.show();
    return app.exec();
}
