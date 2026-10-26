pragma ComponentBehavior: Bound
import QtCore
import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import QtQuick.Layouts

// Settings: sign in, sync folder location, which top-level folders sync.
ApplicationWindow {
    id: win

    required property var backend
    readonly property var accounts: backend.state.accounts || []
    property string account: ""
    readonly property var acct: accounts.find(a => a.name === account) || null
    readonly property string authUrl: acct ? acct.authUrl : ""

    // editable copies, loaded from the daemon
    property string syncDir: ""
    property var folders: []
    property string original: ""
    property string message: ""
    property bool messageIsError: false

    readonly property var excluded: folders.filter(f => f.excluded).map(f => f.name)
    readonly property bool dirty: acct !== null && (syncDir !== acct.syncDir || JSON.stringify(excluded) !== original)

    title: qsTr("OneDrive settings")
    width: 520
    height: 620
    minimumWidth: 420
    minimumHeight: 520

    function load() {
        const f = backend.folders(account)
        syncDir = f.syncDir || ""
        folders = f.folders || []
        original = JSON.stringify(excluded)
    }
    function say(text, isError) { message = text; messageIsError = isError }

    onAuthUrlChanged: if (authUrl) Qt.openUrlExternally(authUrl)
    // opened before the daemon's first answer (e.g. `onedrive-gui --settings` at startup)
    onAccountsChanged: if (!acct && accounts.length) { account = accounts[0].name; load() }

    Connections {
        target: win.backend
        function onSettingsRequested(name) {
            win.account = name || (win.accounts.length ? win.accounts[0].name : "")
            win.message = ""
            win.load()
            win.show()
            win.raise()
            win.requestActivate()
        }
        function onFailed(text) { if (win.visible) win.say(text, true) }
        function onSettingsApplied() {
            win.load()
            win.say(qsTr("Saved. OneDrive is re-checking your files against OneDrive."), false)
        }
    }

    function addAccount() { newName.text = ""; newDir.text = ""; adder.open() }

    Connections {
        target: win.backend
        function onAccountAdded(name) {
            win.account = name
            win.load()
            win.say("", false)
            win.backend.signIn(name)
        }
    }

    Dialog {
        id: adder
        anchors.centerIn: parent
        width: Math.min(parent.width - 32, 440)
        modal: true
        title: qsTr("Add an account")
        standardButtons: Dialog.Ok | Dialog.Cancel
        readonly property string home: decodeURIComponent(StandardPaths.writableLocation(StandardPaths.HomeLocation).toString().replace(/^file:\/\//, ""))
        readonly property string dir: newDir.text || (newName.text ? home + "/OneDrive_" + newName.text : "")
        ColumnLayout {
            width: parent.width
            Label { text: qsTr("Microsoft account email") }
            TextField { id: newName; Layout.fillWidth: true; placeholderText: "name@outlook.com"; inputMethodHints: Qt.ImhEmailCharactersOnly }
            Label { text: qsTr("Folder on this PC (must be empty or new)") }
            RowLayout {
                Layout.fillWidth: true
                TextField { id: newDir; Layout.fillWidth: true; placeholderText: adder.dir }
                Button { text: qsTr("Browse…"); onClicked: newPicker.open() }
            }
        }
        onAccepted: if (newName.text) win.backend.addAccount(newName.text.trim(), adder.dir)
    }

    FolderDialog {
        id: newPicker
        title: qsTr("Choose an empty folder for OneDrive")
        onAccepted: newDir.text = decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, ""))
    }

    FolderDialog {
        id: picker
        title: qsTr("Choose an empty folder for OneDrive")
        onAccepted: win.syncDir = decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, ""))
    }

    Dialog {
        id: confirm
        anchors.centerIn: parent
        width: Math.min(parent.width - 32, 420)
        modal: true
        title: qsTr("Apply changes?")
        standardButtons: Dialog.Ok | Dialog.Cancel
        Label {
            width: parent.width
            wrapMode: Text.Wrap
            text: qsTr("OneDrive will pause, save the settings, then re-check every file against OneDrive. "
                       + "Nothing is deleted on either side; if a file differs, both copies are kept.")
                  + (win.acct && win.syncDir !== win.acct.syncDir
                     ? "\n\n" + qsTr("Your files will be downloaded to the new folder. The current folder is left as it is.") : "")
        }
        onAccepted: {
            win.say(qsTr("Applying… waiting for OneDrive to pause."), false)
            win.backend.applySettings(win.account, win.syncDir, win.excluded)
        }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 16
        spacing: 12

        RowLayout {
            Layout.fillWidth: true
            ComboBox {
                Layout.fillWidth: true
                visible: win.accounts.length > 1
                model: win.accounts.map(a => a.name)
                currentIndex: model.indexOf(win.account)
                onActivated: { win.account = currentText; win.message = ""; win.load() }
            }
            Item { Layout.fillWidth: true; visible: win.accounts.length <= 1 }
            Button { text: qsTr("Add account…"); onClicked: win.addAccount() }
        }

        GroupBox {
            title: qsTr("Account")
            Layout.fillWidth: true
            ColumnLayout {
                anchors.fill: parent
                Label { text: win.account || qsTr("No account yet. Add one to start syncing."); font.weight: Font.DemiBold }

                Label {
                    visible: win.acct !== null && win.acct.status === "signedout"
                    Layout.fillWidth: true; wrapMode: Text.Wrap
                    text: qsTr("You're signed out. Sign in to keep syncing.")
                          + (win.acct && win.acct.error ? "\n" + win.acct.error : "")
                }
                Button {
                    visible: win.acct !== null && win.acct.status === "signedout"
                    text: qsTr("Sign in…")
                    onClicked: { win.message = ""; win.backend.signIn(win.account) }
                }

                Label {
                    visible: win.acct !== null && win.acct.status === "signingin" && !win.authUrl
                    text: qsTr("Starting sign-in…")
                }
                ColumnLayout {
                    visible: win.acct !== null && win.acct.status === "signingin" && win.authUrl !== ""
                    Layout.fillWidth: true
                    Label {
                        Layout.fillWidth: true; wrapMode: Text.Wrap
                        text: qsTr("1. Sign in to Microsoft in the browser page that just opened.\n"
                                   + "2. Your browser then shows a blank page. Copy its full address and paste it here:")
                    }
                    TextField {
                        id: response
                        Layout.fillWidth: true
                        placeholderText: "https://login.microsoftonline.com/common/oauth2/nativeclient?code=…"
                    }
                    RowLayout {
                        Button {
                            text: qsTr("Finish sign-in")
                            enabled: response.text.length > 0
                            onClicked: { win.message = ""; win.backend.signInResponse(win.account, response.text); response.clear() }
                        }
                        Button { text: qsTr("Open page again"); flat: true; onClicked: Qt.openUrlExternally(win.authUrl) }
                        Button { text: qsTr("Cancel"); flat: true; onClicked: win.backend.pause(win.account, 0) }
                    }
                }

                Label {
                    visible: win.acct !== null && !["signedout", "signingin"].includes(win.acct.status)
                    text: qsTr("Signed in")
                    opacity: 0.7
                }
            }
        }

        GroupBox {
            title: qsTr("Folder on this PC")
            Layout.fillWidth: true
            enabled: win.acct !== null && win.acct.status !== "signingin"
            ColumnLayout {
                anchors.fill: parent
                RowLayout {
                    Layout.fillWidth: true
                    Label { Layout.fillWidth: true; text: win.syncDir; elide: Text.ElideMiddle }
                    Button { text: qsTr("Change location…"); onClicked: picker.open() }
                }
                Label {
                    visible: win.acct !== null && win.syncDir !== win.acct.syncDir
                    Layout.fillWidth: true; wrapMode: Text.Wrap; opacity: 0.7
                    text: qsTr("Must be an empty folder. Files are downloaded there; the current folder is left as it is.")
                }
            }
        }

        GroupBox {
            title: qsTr("Folders to sync")
            Layout.fillWidth: true
            Layout.fillHeight: true
            enabled: win.acct !== null && win.acct.status !== "signingin"
            ColumnLayout {
                anchors.fill: parent
                Label {
                    Layout.fillWidth: true; wrapMode: Text.Wrap; opacity: 0.7
                    text: qsTr("Unchecked folders stay on this PC but stop syncing.")
                }
                ListView {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    model: win.folders
                    delegate: CheckBox {
                        required property var modelData
                        required property int index
                        text: modelData.name
                        checked: !modelData.excluded
                        enabled: modelData.selectable
                        onToggled: {
                            const copy = win.folders.slice()
                            copy[index] = Object.assign({}, modelData, { excluded: !checked })
                            win.folders = copy
                        }
                    }
                }
            }
        }

        Label {
            Layout.fillWidth: true
            visible: win.message !== ""
            wrapMode: Text.Wrap
            text: win.message
            color: win.messageIsError ? "#c42b1c" : win.palette.windowText
        }

        RowLayout {
            Layout.alignment: Qt.AlignRight
            Button { text: qsTr("Revert"); enabled: win.dirty; onClicked: { win.message = ""; win.load() } }
            Button { text: qsTr("Apply"); highlighted: true; enabled: win.dirty; onClicked: confirm.open() }
            Button { text: qsTr("Close"); onClicked: win.close() }
        }
    }
}
