// Loads the built plugin and prints the overlays for each path argument once the daemon's state has arrived.
#include <KOverlayIconPlugin>
#include <KPluginFactory>
#include <KPluginMetaData>
#include <QCoreApplication>
#include <QTimer>
#include <QUrl>
#include <cstdio>

int main(int argc, char *argv[])
{
    QCoreApplication app(argc, argv);
    const QStringList args = app.arguments().mid(1);
    auto plugin = KPluginFactory::instantiatePlugin<KOverlayIconPlugin>(KPluginMetaData(args.first())).plugin;
    if (!plugin)
        return 1;
    for (const auto &p : args.mid(1))
        plugin->getOverlays(QUrl::fromLocalFile(p)); // before state: registers the item for updates
    QTimer::singleShot(1000, [&] {
        for (const auto &p : args.mid(1))
            std::printf("%s %s\n", qPrintable(p), qPrintable(plugin->getOverlays(QUrl::fromLocalFile(p)).join(u',')));
        app.quit();
    });
    return app.exec();
}
