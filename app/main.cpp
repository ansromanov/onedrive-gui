// Tray icon + activity flyout. All sync state comes from onedrive-gui-daemon over D-Bus;
// this process never touches onedrive, its config or the synced files.

#include <QApplication>
#include <QCursor>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusPendingCallWatcher>
#include <QDBusServiceWatcher>
#include <QDesktopServices>
#include <QElapsedTimer>
#include <QJsonDocument>
#include <QMenu>
#include <QQmlApplicationEngine>
#include <QQuickWindow>
#include <QScreen>
#include <QSystemTrayIcon>
#include <QUrl>

#ifdef HAVE_LAYERSHELL
#include <LayerShellQt/window.h>
#endif

#include <algorithm>
#include <functional>

static const QString Service = QStringLiteral("org.onedrivegui.Daemon");
static const QString Path = QStringLiteral("/org/onedrivegui/Daemon");
static const QString Iface = QStringLiteral("org.onedrivegui.Daemon1");

class Backend : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QVariantMap state READ state NOTIFY changed)

public:
    Backend()
        : m_watcher(Service, QDBusConnection::sessionBus())
    {
        QDBusConnection::sessionBus().connect(Service, Path, Iface, QStringLiteral("Changed"), this, SLOT(refresh()));
        connect(&m_watcher, &QDBusServiceWatcher::serviceOwnerChanged, this, &Backend::refresh);
        refresh();
    }

    QVariantMap state() const { return m_state; }

    // offline | signedout | error | syncing | paused | synced, worst account wins
    QString overall() const
    {
        const auto accounts = m_state.value(QStringLiteral("accounts")).toList();
        if (accounts.isEmpty())
            return QStringLiteral("offline");
        QStringList statuses;
        for (const auto &a : accounts)
            statuses << a.toMap().value(QStringLiteral("status")).toString();
        if (statuses.contains(QStringLiteral("signedout")) || statuses.contains(QStringLiteral("signingin")))
            return QStringLiteral("signedout");
        if (statuses.contains(QStringLiteral("error")) || statuses.contains(QStringLiteral("stopped")))
            return QStringLiteral("error");
        if (statuses.contains(QStringLiteral("syncing")) || statuses.contains(QStringLiteral("starting")))
            return QStringLiteral("syncing");
        if (statuses.count(QStringLiteral("paused")) == statuses.size())
            return QStringLiteral("paused");
        return QStringLiteral("synced");
    }

    Q_INVOKABLE void pause(const QString &account, uint seconds) { call(QStringLiteral("Pause"), {account, seconds}); }
    Q_INVOKABLE void resume(const QString &account) { call(QStringLiteral("Resume"), {account}); }
    Q_INVOKABLE void open(const QString &localPath) { QDesktopServices::openUrl(QUrl::fromLocalFile(localPath)); }
    Q_INVOKABLE void signIn(const QString &account) { call(QStringLiteral("SignIn"), {account}); }
    Q_INVOKABLE void signInResponse(const QString &account, const QString &url) { call(QStringLiteral("SignInResponse"), {account, url}); }
    Q_INVOKABLE void openSettings(const QString &account) { emit settingsRequested(account); }

    Q_INVOKABLE QVariantMap folders(const QString &account)
    {
        auto msg = QDBusMessage::createMethodCall(Service, Path, Iface, QStringLiteral("Folders"));
        msg << account;
        const QDBusMessage reply = QDBusConnection::sessionBus().call(msg);
        if (reply.type() != QDBusMessage::ReplyMessage) {
            emit failed(reply.errorMessage());
            return {};
        }
        return QJsonDocument::fromJson(reply.arguments().value(0).toString().toUtf8()).toVariant().toMap();
    }

    Q_INVOKABLE void applySettings(const QString &account, const QString &syncDir, const QStringList &excluded)
    {
        // the daemon waits for onedrive to stop cleanly first, which can take a while mid-transfer
        call(QStringLiteral("ApplySettings"), {account, syncDir, excluded}, [this](const QDBusMessage &reply) {
            if (reply.type() == QDBusMessage::ReplyMessage)
                emit settingsApplied();
        }, 10 * 60 * 1000);
    }

signals:
    void changed();
    void failed(const QString &message);
    void settingsApplied();
    void settingsRequested(const QString &account);

public slots:
    void refresh()
    {
        call(QStringLiteral("State"), {}, [this](const QDBusMessage &reply) {
            // daemon gone -> empty state, shown as "not running"
            m_state = reply.type() == QDBusMessage::ReplyMessage
                ? QJsonDocument::fromJson(reply.arguments().value(0).toString().toUtf8()).toVariant().toMap()
                : QVariantMap();
            emit changed();
        });
    }

private:
    void call(const QString &method, const QVariantList &args, std::function<void(const QDBusMessage &)> done = {}, int timeout = -1)
    {
        auto msg = QDBusMessage::createMethodCall(Service, Path, Iface, method);
        msg.setArguments(args);
        auto *w = new QDBusPendingCallWatcher(QDBusConnection::sessionBus().asyncCall(msg, timeout), this);
        connect(w, &QDBusPendingCallWatcher::finished, this, [this, w, method, done] {
            w->deleteLater();
            const QDBusMessage reply = w->reply();
            if (reply.type() == QDBusMessage::ErrorMessage && method != QStringLiteral("State"))
                emit failed(reply.errorMessage());
            if (done)
                done(reply);
        });
    }

    QDBusServiceWatcher m_watcher;
    QVariantMap m_state;
};

static bool useLayerShell(QWindow *win)
{
#ifdef HAVE_LAYERSHELL
    // GNOME (Mutter) has no wlr-layer-shell; it gets a normal window there.
    if (QGuiApplication::platformName() != QStringLiteral("wayland")
        || qEnvironmentVariable("XDG_CURRENT_DESKTOP").contains(QStringLiteral("GNOME")))
        return false;
    auto *lw = LayerShellQt::Window::get(win);
    lw->setLayer(LayerShellQt::Window::LayerTop);
    // shortcut: always bottom-right like Windows; read the panel position if top panels need it
    lw->setAnchors({LayerShellQt::Window::AnchorBottom | LayerShellQt::Window::AnchorRight});
    lw->setMargins({0, 0, 12, 12});
    lw->setKeyboardInteractivity(LayerShellQt::Window::KeyboardInteractivityOnDemand);
    lw->setScope(QStringLiteral("onedrive-gui"));
    return true;
#else
    Q_UNUSED(win);
    return false;
#endif
}

// X11: next to the cursor, which sits on the tray icon right after the click.
static void placeNearCursor(QWindow *win)
{
    const QPoint c = QCursor::pos();
    QScreen *screen = QGuiApplication::screenAt(c);
    if (!screen)
        return;
    const QRect g = screen->availableGeometry();
    const int x = std::clamp(c.x() - win->width() / 2, g.left() + 8, g.right() - win->width() - 8);
    const int y = c.y() > g.center().y() ? g.bottom() - win->height() - 8 : g.top() + 8;
    win->setPosition(x, y);
}

static QIcon iconFor(const QString &overall)
{
    static const QHash<QString, QString> names{
        {QStringLiteral("synced"), QStringLiteral("state-ok")},
        {QStringLiteral("syncing"), QStringLiteral("state-sync")},
        {QStringLiteral("paused"), QStringLiteral("state-pause")},
        {QStringLiteral("error"), QStringLiteral("state-error")},
        {QStringLiteral("signedout"), QStringLiteral("state-offline")},
        {QStringLiteral("offline"), QStringLiteral("state-offline")},
    };
    return QIcon::fromTheme(names.value(overall), QIcon::fromTheme(QStringLiteral("folder-cloud"), QIcon::fromTheme(QStringLiteral("folder"))));
}

static QString tooltipFor(const QString &overall)
{
    if (overall == QStringLiteral("synced"))
        return QObject::tr("Your files are synced");
    if (overall == QStringLiteral("syncing"))
        return QObject::tr("Syncing files…");
    if (overall == QStringLiteral("paused"))
        return QObject::tr("Sync is paused");
    if (overall == QStringLiteral("error"))
        return QObject::tr("Sync problem");
    if (overall == QStringLiteral("signedout"))
        return QObject::tr("You're signed out");
    return QObject::tr("OneDrive GUI daemon isn't running");
}

int main(int argc, char *argv[])
{
    QApplication app(argc, argv);
    app.setApplicationName(QStringLiteral("onedrive-gui"));
    app.setQuitOnLastWindowClosed(false);

    Backend backend;
    QQmlApplicationEngine engine;
    engine.setInitialProperties({{QStringLiteral("backend"), QVariant::fromValue(&backend)}});
    engine.loadFromModule("OneDriveGui", "Main");
    engine.loadFromModule("OneDriveGui", "Settings");
    if (engine.rootObjects().size() != 2)
        return 1;
    auto *win = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
    const bool layerShell = useLayerShell(win);

    // A tray click first deactivates (hides) the open flyout; don't reopen it on that same click.
    QElapsedTimer sinceHide;
    sinceHide.start();
    QObject::connect(win, &QWindow::visibleChanged, [&](bool visible) {
        if (!visible)
            sinceHide.restart();
    });
    auto toggle = [&] {
        if (win->isVisible() || sinceHide.elapsed() < 300) {
            win->hide();
            return;
        }
        if (!layerShell && QGuiApplication::platformName() == QStringLiteral("xcb"))
            placeNearCursor(win);
        win->show();
        win->raise();
        win->requestActivate();
    };

    QSystemTrayIcon tray;
    QMenu menu;
    menu.addAction(QObject::tr("Show activity"), toggle);
    menu.addAction(QObject::tr("Settings"), [&] { backend.openSettings({}); });
    menu.addAction(QObject::tr("Quit OneDrive GUI (sync keeps running)"), &app, &QApplication::quit);
    tray.setContextMenu(&menu);
    QObject::connect(&tray, &QSystemTrayIcon::activated, [&](QSystemTrayIcon::ActivationReason reason) {
        if (reason == QSystemTrayIcon::Trigger)
            toggle();
    });
    auto updateTray = [&] {
        const QString overall = backend.overall();
        tray.setIcon(iconFor(overall));
        tray.setToolTip(QStringLiteral("OneDrive – ") + tooltipFor(overall));
    };
    QObject::connect(&backend, &Backend::changed, updateTray);
    updateTray();
    tray.show();

    return app.exec();
}

#include "main.moc"
