pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Activity flyout, modelled on the Windows OneDrive one: status per account, recent files, pause menu.
Window {
    id: root

    required property var backend
    readonly property var accounts: backend.state.accounts || []
    readonly property var recent: backend.state.recent || []
    property real now: Date.now() / 1000

    width: 360
    height: 520
    color: "transparent"
    flags: Qt.FramelessWindowHint | Qt.Tool
    title: "OneDrive"

    onActiveChanged: if (!active) visible = false

    function statusText(a) {
        switch (a.status) {
        case "synced": return qsTr("Your files are synced")
        case "syncing": return qsTr("Syncing files…")
        case "starting": return qsTr("Starting…")
        case "paused":
            return a.pausedUntil > 0
                ? qsTr("Paused until %1").arg(new Date(a.pausedUntil * 1000).toLocaleTimeString(Qt.locale(), Locale.ShortFormat))
                : qsTr("Sync is paused")
        case "stopped": return qsTr("Sync stopped: %1").arg(a.error)
        case "signedout": return qsTr("You're signed out")
        case "signingin": return qsTr("Signing in…")
        default: return a.error || qsTr("Sync problem")
        }
    }
    function statusGlyph(s) { return ({ synced: "✓", syncing: "⟳", starting: "⟳", paused: "⏸" })[s] || "!" }
    function statusColor(s) {
        if (s === "synced") return "#107c10"
        if (s === "paused" || s === "signedout" || s === "signingin") return "#9d5d00"
        if (s === "error" || s === "stopped") return "#c42b1c"
        return palette.highlight
    }
    function verb(r) {
        if (!r.ok) return qsTr("Couldn't sync")
        return ({ "upload": qsTr("Uploaded"), "download": qsTr("Downloaded"),
                  "conflict": qsTr("Conflict: kept both copies, this is your local one"),
                  "delete-remote": qsTr("Deleted from OneDrive"), "delete-local": qsTr("Deleted from this PC") })[r.action]
    }
    function ago(t) {
        const d = Math.max(0, now - t)
        if (d < 60) return qsTr("Just now")
        if (d < 3600) return qsTr("%1 min ago").arg(Math.floor(d / 60))
        if (d < 86400) return qsTr("%1 h ago").arg(Math.floor(d / 3600))
        return new Date(t * 1000).toLocaleDateString(Qt.locale(), Locale.ShortFormat)
    }
    function cleanPath(p) { return p.startsWith("./") ? p.slice(2) : p }
    function localPath(r) {
        const a = accounts.find(x => x.name === r.account)
        return a ? a.syncDir + "/" + cleanPath(r.path) : ""
    }

    Timer {
        interval: 30000; repeat: true; triggeredOnStart: true
        running: root.visible
        onTriggered: root.now = Date.now() / 1000
    }
    Shortcut { sequence: "Escape"; onActivated: root.visible = false }

    Rectangle {
        anchors.fill: parent
        radius: 8
        color: root.palette.window
        border.color: Qt.rgba(root.palette.windowText.r, root.palette.windowText.g, root.palette.windowText.b, 0.15)

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 1
            spacing: 0

            RowLayout {
                Layout.fillWidth: true
                Layout.margins: 12
                Label { text: "OneDrive"; font.pixelSize: 16; font.weight: Font.DemiBold; Layout.fillWidth: true }
                ToolButton {
                    text: qsTr("Open folder"); icon.name: "folder"; display: AbstractButton.IconOnly
                    ToolTip.visible: hovered; ToolTip.text: text
                    enabled: root.accounts.length > 0
                    onClicked: { root.backend.open(root.accounts[0].syncDir); root.visible = false }
                }
                ToolButton {
                    text: qsTr("View online"); icon.name: "internet-web-browser"; display: AbstractButton.IconOnly
                    ToolTip.visible: hovered; ToolTip.text: text
                    onClicked: { Qt.openUrlExternally("https://onedrive.live.com"); root.visible = false }
                }
                ToolButton {
                    id: gear
                    text: qsTr("Help & Settings"); icon.name: "preferences-system"; display: AbstractButton.IconOnly
                    ToolTip.visible: hovered; ToolTip.text: text
                    onClicked: menu.popup(gear, 0, gear.height)
                }
            }

            Repeater {
                model: root.accounts
                delegate: RowLayout {
                    id: acct
                    required property var modelData
                    Layout.fillWidth: true
                    Layout.leftMargin: 12; Layout.rightMargin: 12; Layout.bottomMargin: 10
                    spacing: 10

                    Rectangle {
                        Layout.preferredWidth: 28; Layout.preferredHeight: 28; radius: 14
                        color: root.statusColor(acct.modelData.status)
                        Label { anchors.centerIn: parent; text: root.statusGlyph(acct.modelData.status); color: "white"; font.bold: true }
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 0
                        Label {
                            Layout.fillWidth: true
                            text: root.statusText(acct.modelData); font.weight: Font.DemiBold
                            wrapMode: Text.Wrap; maximumLineCount: 2; elide: Text.ElideRight
                        }
                        Label { Layout.fillWidth: true; text: acct.modelData.name; opacity: 0.7; font.pixelSize: 12; elide: Text.ElideRight }
                    }
                    Button {
                        visible: acct.modelData.status === "paused" || acct.modelData.status === "stopped"
                        text: acct.modelData.status === "paused" ? qsTr("Resume") : qsTr("Retry")
                        onClicked: root.backend.resume(acct.modelData.name)
                    }
                    Button {
                        visible: acct.modelData.status === "signedout" || acct.modelData.status === "signingin"
                        text: qsTr("Sign in")
                        onClicked: { root.backend.openSettings(acct.modelData.name); root.visible = false }
                    }
                }
            }

            Rectangle { Layout.fillWidth: true; Layout.preferredHeight: 1; color: Qt.rgba(root.palette.windowText.r, root.palette.windowText.g, root.palette.windowText.b, 0.12) }

            ListView {
                id: list
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                model: root.recent
                delegate: ItemDelegate {
                    id: row
                    required property var modelData
                    readonly property bool openable: modelData.ok && ["upload", "download", "conflict"].includes(modelData.action)
                    width: ListView.view.width
                    hoverEnabled: openable
                    onClicked: if (openable) { root.backend.open(root.localPath(modelData)); root.visible = false }
                    contentItem: RowLayout {
                        spacing: 10
                        Label {
                            Layout.preferredWidth: 24
                            horizontalAlignment: Text.AlignHCenter
                            font.pixelSize: 18
                            text: ({ upload: "↑", download: "↓", conflict: "⚠" })[row.modelData.action] || "✕"
                            color: !row.modelData.ok ? "#c42b1c" : row.modelData.action === "conflict" ? "#9d5d00" : root.palette.highlight
                        }
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 0
                            Label { Layout.fillWidth: true; text: root.cleanPath(row.modelData.path).split("/").pop(); elide: Text.ElideMiddle }
                            Label {
                                Layout.fillWidth: true; opacity: 0.7; font.pixelSize: 12; elide: Text.ElideRight
                                text: root.verb(row.modelData) + " · " + root.ago(row.modelData.time)
                            }
                        }
                    }
                }
                Label {
                    anchors.centerIn: parent
                    visible: list.count === 0
                    opacity: 0.7
                    text: root.accounts.length ? qsTr("No recent activity") : qsTr("The OneDrive GUI daemon isn't running")
                }
            }
        }
    }

    Menu {
        id: menu
        Menu {
            title: qsTr("Pause syncing")
            Repeater {
                model: [2, 8, 24]
                MenuItem {
                    required property int modelData
                    text: qsTr("%1 hours").arg(modelData)
                    onTriggered: root.accounts.forEach(a => root.backend.pause(a.name, modelData * 3600))
                }
            }
        }
        MenuItem {
            text: qsTr("Resume syncing")
            enabled: root.accounts.some(a => a.status === "paused")
            onTriggered: root.accounts.forEach(a => { if (a.status === "paused") root.backend.resume(a.name) })
        }
        MenuItem {
            text: qsTr("Settings")
            onTriggered: { root.backend.openSettings(""); root.visible = false }
        }
        MenuSeparator {}
        MenuItem { text: qsTr("Quit OneDrive GUI (sync keeps running)"); onTriggered: Qt.quit() }
    }
}
