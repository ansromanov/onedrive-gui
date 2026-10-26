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
#include <QFile>
#include <QJsonDocument>
#include <QMenu>
#include <QQmlApplicationEngine>
#include <QQuickWindow>
#include <QRegularExpression>
#include <QScreen>
#include <QSet>
#include <QStandardPaths>
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
static const QString AppService = QStringLiteral("org.onedrivegui.App");

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
    Q_INVOKABLE void addAccount(const QString &account, const QString &syncDir)
    {
        call(QStringLiteral("AddAccount"), {account, syncDir}, [this, account](const QDBusMessage &reply) {
            if (reply.type() == QDBusMessage::ReplyMessage)
                emit accountAdded(account);
        });
    }
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
    void accountAdded(const QString &account);

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

// Lets a second launch (e.g. `onedrive-gui --settings`) reach the running instance instead of adding a second tray icon.
class App : public QObject
{
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "org.onedrivegui.App1")
public:
    std::function<void()> activity, settings;
public slots:
    Q_SCRIPTABLE void ShowActivity() { activity(); }
    Q_SCRIPTABLE void ShowSettings() { settings(); }
};

// Panel edge the tray lives on: from the screen's reserved area where the platform reports it (X11),
// else from Plasma's panel config (Wayland reports none). Qt::BottomEdge if unknown.
static Qt::Edge panelEdge()
{
    if (QScreen *s = QGuiApplication::primaryScreen(); s && s->availableGeometry() != s->geometry()) {
        const QRect g = s->geometry(), a = s->availableGeometry();
        if (a.top() > g.top())
            return Qt::TopEdge;
        if (a.left() > g.left())
            return Qt::LeftEdge;
        if (a.right() < g.right())
            return Qt::RightEdge;
        return Qt::BottomEdge;
    }
    // [Containments][N] location=3 top, 4 bottom, 5 left, 6 right; the panel holding the system tray wins
    QFile f(QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation) + QStringLiteral("/plasma-org.kde.plasma.desktop-appletsrc"));
    if (!f.open(QIODevice::ReadOnly))
        return Qt::BottomEdge;
    static const QRegularExpression group(QStringLiteral(R"(^\[Containments\]\[(\d+)\](.*)$)"));
    QHash<QString, int> location;
    QString current, trayPanel;
    bool inContainment = false;
    while (!f.atEnd()) {
        const QString line = QString::fromUtf8(f.readLine()).trimmed();
        if (line.startsWith(u'[')) {
            const auto m = group.match(line);
            current = m.hasMatch() ? m.captured(1) : QString();
            inContainment = m.hasMatch() && m.captured(2).isEmpty();
        } else if (inContainment && line.startsWith(QStringLiteral("location="))) {
            location[current] = line.mid(9).toInt();
        } else if (!current.isEmpty() && line == QStringLiteral("plugin=org.kde.plasma.systemtray")) {
            trayPanel = current;
        }
    }
    switch (location.value(trayPanel, 4)) {
    case 3: return Qt::TopEdge;
    case 5: return Qt::LeftEdge;
    case 6: return Qt::RightEdge;
    default: return Qt::BottomEdge;
    }
}

static bool useLayerShell(QWindow *win)
{
#ifdef HAVE_LAYERSHELL
    // GNOME (Mutter) has no wlr-layer-shell; it gets a normal window there.
    if (QGuiApplication::platformName() != QStringLiteral("wayland")
        || qEnvironmentVariable("XDG_CURRENT_DESKTOP").contains(QStringLiteral("GNOME")))
        return false;
    auto *lw = LayerShellQt::Window::get(win);
    lw->setLayer(LayerShellQt::Window::LayerTop);
    // in the corner next to the tray, right side like Windows unless the panel is on the left
    const Qt::Edge edge = panelEdge();
    lw->setAnchors({(edge == Qt::TopEdge ? LayerShellQt::Window::AnchorTop : LayerShellQt::Window::AnchorBottom)
                    | (edge == Qt::LeftEdge ? LayerShellQt::Window::AnchorLeft : LayerShellQt::Window::AnchorRight)});
    lw->setMargins({12, 12, 12, 12});
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

static QString fileName(const QVariantMap &r)
{
    return r.value(QStringLiteral("path")).toString().section(u'/', -1);
}

// Desktop notifications for conflicts, failed files and accounts that stop syncing; `warned` holds accounts
// already notified about, until they sync again, so a crash-restart loop notifies once.
static void notifyChanges(const QVariantMap &before, const QVariantMap &after, QSet<QString> &warned, QSystemTrayIcon &tray)
{
    // first state after the app or the daemon starts: nothing in it is new
    if (before.isEmpty() || after.isEmpty())
        return;
    const auto oldRecent = before.value(QStringLiteral("recent")).toList();
    const QVariant top = oldRecent.isEmpty() ? QVariant() : oldRecent.first();
    QStringList conflicts, failures;
    for (const auto &v : after.value(QStringLiteral("recent")).toList()) {
        if (v == top)
            break;
        const auto r = v.toMap();
        if (!r.value(QStringLiteral("ok")).toBool())
            failures << fileName(r);
        else if (r.value(QStringLiteral("action")) == QStringLiteral("conflict"))
            conflicts << fileName(r);
    }
    QStringList messages;
    if (conflicts.size() == 1)
        messages << QObject::tr("Conflict in %1: both copies were kept.").arg(conflicts.first());
    else if (conflicts.size() > 1)
        messages << QObject::tr("%1 files had conflicts: both copies were kept.").arg(conflicts.size());
    if (failures.size() == 1)
        messages << QObject::tr("Couldn't sync %1.").arg(failures.first());
    else if (failures.size() > 1)
        messages << QObject::tr("Couldn't sync %1 files.").arg(failures.size());

    for (const auto &v : after.value(QStringLiteral("accounts")).toList()) {
        const auto a = v.toMap();
        const QString name = a.value(QStringLiteral("name")).toString(), status = a.value(QStringLiteral("status")).toString();
        if (status == QStringLiteral("synced") || status == QStringLiteral("paused") || status == QStringLiteral("syncing")) {
            warned.remove(name);
            continue;
        }
        if (warned.contains(name))
            continue;
        if (status == QStringLiteral("signedout"))
            messages << QObject::tr("You're signed out of %1. Sign in to keep syncing.").arg(name);
        else if (status == QStringLiteral("stopped") || status == QStringLiteral("error"))
            messages << QObject::tr("Sync problem with %1: %2").arg(name, a.value(QStringLiteral("error")).toString());
        else
            continue;
        warned.insert(name);
    }
    if (!messages.isEmpty())
        tray.showMessage(QStringLiteral("OneDrive"), messages.join(u'\n'), QSystemTrayIcon::Warning);
}

int main(int argc, char *argv[])
{
    QApplication app(argc, argv);
    app.setApplicationName(QStringLiteral("onedrive-gui"));
    app.setQuitOnLastWindowClosed(false);
    const bool wantSettings = app.arguments().contains(QStringLiteral("--settings"));

    // already running: hand the request to it
    if (!QDBusConnection::sessionBus().registerService(AppService)) {
        auto msg = QDBusMessage::createMethodCall(AppService, QStringLiteral("/App"), QStringLiteral("org.onedrivegui.App1"),
                                                  wantSettings ? QStringLiteral("ShowSettings") : QStringLiteral("ShowActivity"));
        QDBusConnection::sessionBus().call(msg);
        return 0;
    }

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

    App appObject;
    appObject.activity = [&] {
        if (!win->isVisible())
            toggle();
    };
    appObject.settings = [&] { backend.openSettings({}); };
    QDBusConnection::sessionBus().registerObject(QStringLiteral("/App"), &appObject, QDBusConnection::ExportScriptableSlots);

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
    QVariantMap lastState;
    QSet<QString> warned;
    auto updateTray = [&] {
        const QString overall = backend.overall();
        tray.setIcon(iconFor(overall));
        tray.setToolTip(QStringLiteral("OneDrive – ") + tooltipFor(overall));
        notifyChanges(lastState, backend.state(), warned, tray);
        lastState = backend.state();
    };
    QObject::connect(&backend, &Backend::changed, updateTray);
    QObject::connect(&tray, &QSystemTrayIcon::messageClicked, appObject.activity);
    updateTray();
    tray.show();
    if (wantSettings)
        backend.openSettings({});

    return app.exec();
}

#include "main.moc"
