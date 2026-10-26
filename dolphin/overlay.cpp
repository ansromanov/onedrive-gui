// Dolphin overlay icons for files in OneDrive folders. Read-only: state comes from onedrive-gui-daemon over D-Bus.
//
// onedrive keeps its database locked while it runs, so there is no per-file record to read. An item shows
// a check mark when its account is fully synced, unless it is excluded, skipped by onedrive's default
// skip_file patterns, or failed to sync; the sync folder itself shows the account's state.
// shortcut: custom skip_file/skip_dir patterns from the config aren't applied; read them if wrong marks show up.

#include <KOverlayIconPlugin>
#include <KPluginFactory>

#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusPendingCallWatcher>
#include <QDBusServiceWatcher>
#include <QJsonDocument>
#include <QRegularExpression>
#include <QSet>
#include <QUrl>

static const QString Service = QStringLiteral("org.onedrivegui.Daemon");
static const QString Path = QStringLiteral("/org/onedrivegui/Daemon");
static const QString Iface = QStringLiteral("org.onedrivegui.Daemon1");

class OneDriveOverlay : public KOverlayIconPlugin
{
    Q_OBJECT
public:
    OneDriveOverlay(QObject *parent, const QVariantList &)
        : KOverlayIconPlugin(parent)
        , m_watcher(Service, QDBusConnection::sessionBus())
    {
        QDBusConnection::sessionBus().connect(Service, Path, Iface, QStringLiteral("Changed"), this, SLOT(refresh()));
        connect(&m_watcher, &QDBusServiceWatcher::serviceOwnerChanged, this, &OneDriveOverlay::refresh);
        refresh();
    }

    QStringList getOverlays(const QUrl &url) override
    {
        if (!url.isLocalFile())
            return {};
        QStringList overlays = overlaysFor(url.toLocalFile());
        if (m_shown.size() > 20000) // bound memory in long Dolphin sessions; cleared items just stop updating live
            m_shown.clear();
        m_shown.insert(url, overlays);
        return overlays;
    }

private slots:
    void refresh()
    {
        auto *w = new QDBusPendingCallWatcher(
            QDBusConnection::sessionBus().asyncCall(QDBusMessage::createMethodCall(Service, Path, Iface, QStringLiteral("State"))), this);
        connect(w, &QDBusPendingCallWatcher::finished, this, [this, w] {
            w->deleteLater();
            const QDBusMessage reply = w->reply();
            m_state = reply.type() == QDBusMessage::ReplyMessage
                ? QJsonDocument::fromJson(reply.arguments().value(0).toString().toUtf8()).toVariant().toMap()
                : QVariantMap();
            for (auto it = m_shown.begin(); it != m_shown.end(); ++it) {
                const QStringList now = overlaysFor(it.key().toLocalFile());
                if (now != it.value()) {
                    it.value() = now;
                    Q_EMIT overlaysChanged(it.key(), now);
                }
            }
        });
    }

private:
    QStringList overlaysFor(const QString &path) const
    {
        for (const auto &v : m_state.value(QStringLiteral("accounts")).toList()) {
            const auto a = v.toMap();
            const QString root = a.value(QStringLiteral("syncDir")).toString();
            if (root.isEmpty() || (path != root && !path.startsWith(root + u'/')))
                continue;
            const QString status = a.value(QStringLiteral("status")).toString();
            if (path == root)
                return {statusIcon(status)};
            const QString rel = path.mid(root.size() + 1);
            if (a.value(QStringLiteral("excluded")).toStringList().contains(rel.section(u'/', 0, 0)) || skipped(rel))
                return {};
            if (failed(a.value(QStringLiteral("name")).toString(), rel))
                return {QStringLiteral("emblem-error")};
            if (rel.contains(QStringLiteral("-safeBackup-")))
                return {QStringLiteral("emblem-warning")};
            return status == QStringLiteral("synced") ? QStringList{QStringLiteral("emblem-checked")} : QStringList{};
        }
        return {};
    }

    static QString statusIcon(const QString &status)
    {
        if (status == QStringLiteral("synced"))
            return QStringLiteral("emblem-checked");
        if (status == QStringLiteral("syncing") || status == QStringLiteral("starting"))
            return QStringLiteral("emblem-synchronizing-symbolic");
        if (status == QStringLiteral("paused"))
            return QStringLiteral("emblem-pause");
        return QStringLiteral("emblem-error");
    }

    // onedrive's default skip_file: ~*|.~*|*.tmp|*.swp|*.partial
    static bool skipped(const QString &rel)
    {
        static const QRegularExpression re(QStringLiteral(R"(^(~.*|\.~.*|.*\.tmp|.*\.swp|.*\.partial)$)"), QRegularExpression::CaseInsensitiveOption);
        return re.match(rel.section(u'/', -1)).hasMatch();
    }

    // latest result for this file in the recent list is a failure
    bool failed(const QString &account, const QString &rel) const
    {
        for (const auto &v : m_state.value(QStringLiteral("recent")).toList()) {
            const auto r = v.toMap();
            QString p = r.value(QStringLiteral("path")).toString();
            if (p.startsWith(QStringLiteral("./")))
                p = p.mid(2);
            if (r.value(QStringLiteral("account")) == account && p == rel)
                return !r.value(QStringLiteral("ok")).toBool();
        }
        return false;
    }

    QDBusServiceWatcher m_watcher;
    QVariantMap m_state;
    QHash<QUrl, QStringList> m_shown;
};

K_PLUGIN_CLASS_WITH_JSON(OneDriveOverlay, "overlay.json")

#include "overlay.moc"
