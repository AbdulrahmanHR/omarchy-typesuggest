import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import qs.Commons
import qs.Ui
import "Model.js" as Model

// TypeSuggest bar widget: a keyboard icon that dims while the daemon is off,
// and a keyboard-driven panel with the on/off switch and quick settings.
// Built from the same shell pieces as the first-party Tailscale panel (hero
// with a trailing switch, missing-CLI state) and Monitor panel (settings rows,
// preset buttons, notched slider).
Panel {
  id: root
  moduleName: "io.github.abdulrahmanhr.typesuggest"
  ipcTarget: "io.github.abdulrahmanhr.typesuggest"

  // Cursor model shared by keyboard and mouse, as in the first-party panels:
  // focusSection names a row, selectedIndex the control inside it.
  //   install        "Install TypeSuggest" (only while it is missing)
  //   header         the on/off switch in the hero
  //   look           Below, Above | Omarchy, Default (two groups side by side)
  //   size           bar size slider; h/l steps it
  //   count          1 to 5 suggestions
  //   select         Up, Down (only when typesuggest reports select_key)
  //   keys           Enter, Space, Tab
  //   learn, typo    switch rows
  //   clear, config  action rows
  // Mouse hover moves the same cursor, so one highlight shows at a time.
  property string focusSection: "header"
  property int selectedIndex: 0
  property bool cursorActive: false
  property bool clearConfirmOpen: false
  property int phraseIndex: 0

  readonly property var activePhrases: [
    "Predicting words",
    "Finishing sentences",
    "Ranking candidates",
    "Weighing bigrams",
    "Watching the caret",
    "Polishing prefixes",
    "Saving keystrokes"
  ]
  readonly property string heroPhraseText: activePhrases[phraseIndex % activePhrases.length]

  readonly property color foreground: bar ? bar.foreground : Color.foreground
  readonly property color urgent: bar ? bar.urgent : Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family

  readonly property var config: typesuggest.config
  readonly property var scaleStops: Model.barScaleStops()
  readonly property bool settingsReady: typesuggest.installed && typesuggest.supportsSettings && typesuggest.configLoaded
  readonly property bool outdated: typesuggest.installed && typesuggest.capabilityChecked && !typesuggest.supportsSettings
  readonly property string installScriptPath: Model.localPath(Qt.resolvedUrl("scripts/install-typesuggest.sh"))

  // md-keyboard / md-keyboard-off from the bar's Nerd Font, like the
  // microphone widget's muted glyph.
  readonly property string iconGlyph: typesuggest.active ? "󰌌" : "󰌐"
  readonly property color barIconColor: typesuggest.active ? barForeground : Qt.darker(barForeground, 1.55)
  readonly property color iconColor: typesuggest.active ? foreground : dim
  readonly property string toggleHint: typesuggest.active ? "Turn TypeSuggest off" : "Turn TypeSuggest on"
  readonly property string barTooltip: {
    if (!typesuggest.probed) return "TypeSuggest"
    if (!typesuggest.installed) return "TypeSuggest is not installed"
    return typesuggest.active ? "TypeSuggest is on" : "TypeSuggest is off"
  }
  readonly property string heroMeta: {
    if (!typesuggest.probed) return "Checking"
    if (!typesuggest.installed) return "Not installed"
    return typesuggest.active ? heroPhraseText : "Suggestions are off"
  }
  readonly property string statusText: typesuggest.actionStatus !== ""
    ? typesuggest.actionStatus
    : (typesuggest.lastError !== "" ? typesuggest.lastError : typesuggest.configError)
  readonly property bool statusIsError: typesuggest.actionStatus === "" && statusText !== ""
  // Older typesuggest builds report no select_key and always use Up
  readonly property bool selectKeySupported: root.config.select_key === "up" || root.config.select_key === "down"
  readonly property string selectKeyLabel: root.config.select_key === "down" ? "Down" : "Up"
  readonly property string backKeyLabel: root.config.select_key === "down" ? "Up" : "Down"

  readonly property var sections: {
    if (!typesuggest.probed) return []
    if (!typesuggest.installed) return ["install"]
    var list = ["header"]
    if (settingsReady) {
      list = list.concat(["look", "size", "count"])
      if (selectKeySupported) list.push("select")
      list = list.concat(["keys", "learn", "typo"])
    }
    list.push("clear")
    list.push("config")
    return list
  }

  function sectionCount(section) {
    if (section === "look") return 4
    if (section === "count") return 5
    if (section === "select") return 2
    if (section === "keys") return 3
    return 1
  }

  // Entering a row lands on its current choice, so you start from what is set.
  function landingIndex(section) {
    if (section === "look") return root.config.bar_position === "above" ? 1 : 0
    if (section === "count") return Math.max(0, Math.min(4, root.config.max_candidates - 1))
    if (section === "select") return root.config.select_key === "down" ? 1 : 0
    return 0
  }

  function cursorAt(section, index) {
    return root.cursorActive && root.focusSection === section && root.selectedIndex === index
  }

  function setCursor(section, index) {
    cursorActive = true
    focusSection = section
    selectedIndex = index
  }

  function clampCursor() {
    var list = sections
    if (list.length === 0) return
    if (list.indexOf(focusSection) === -1) {
      focusSection = list[0]
      selectedIndex = landingIndex(focusSection)
      return
    }
    selectedIndex = Math.max(0, Math.min(sectionCount(focusSection) - 1, selectedIndex))
  }

  function moveCursor(dx, dy) {
    cursorActive = true
    clampCursor()
    var list = sections
    if (list.length === 0) return
    if (dy !== 0) {
      var next = Math.max(0, Math.min(list.length - 1, list.indexOf(focusSection) + dy))
      if (list[next] !== focusSection) {
        focusSection = list[next]
        selectedIndex = landingIndex(focusSection)
      }
      return
    }
    if (dx === 0) return
    if (focusSection === "size") stepScale(dx)
    else selectedIndex = Math.max(0, Math.min(sectionCount(focusSection) - 1, selectedIndex + dx))
  }

  function activateCursor() {
    clampCursor()
    var i = selectedIndex
    if (focusSection === "install") installTypeSuggest()
    else if (focusSection === "header") typesuggest.toggleService()
    else if (focusSection === "look") {
      if (i < 2) typesuggest.setOption("bar_position", Model.positions()[i])
      else typesuggest.setOption("theme", Model.themes()[i - 2])
    }
    else if (focusSection === "count") typesuggest.setOption("max_candidates", i + 1)
    else if (focusSection === "select") typesuggest.setOption("select_key", Model.selectKeys()[i])
    else if (focusSection === "keys") typesuggest.toggleAcceptKey(Model.acceptKeyNames()[i])
    else if (focusSection === "learn") typesuggest.setOption("learn", !root.config.learn)
    else if (focusSection === "typo") typesuggest.setOption("typo_correction", !root.config.typo_correction)
    else if (focusSection === "clear") openClearConfirm()
    else if (focusSection === "config") openConfigFile()
  }

  function setScale(value) {
    if (Math.abs(Number(root.config.bar_scale) - value) < 0.001) return
    typesuggest.setOption("bar_scale", value)
  }

  function stepScale(delta) {
    setScale(Model.stepScale(root.config.bar_scale, delta))
  }

  function installTypeSuggest() {
    typesuggest.install(root.installScriptPath)
    root.close()
  }

  function openConfigFile() {
    typesuggest.openConfig()
    root.close()
  }

  // Cancel is preselected: clearing cannot be undone.
  function openClearConfirm() {
    clearConfirm.selectedIndex = 0
    root.clearConfirmOpen = true
  }

  function cancelClear() {
    root.clearConfirmOpen = false
  }

  function confirmClear() {
    root.clearConfirmOpen = false
    typesuggest.clearLearned()
  }

  function toggleConfirmChoice() {
    clearConfirm.selectedIndex = clearConfirm.selectedIndex === 0 ? 1 : 0
  }

  function scrollItemIntoView(item) {
    if (!panelFlick || !item) return
    Qt.callLater(function() {
      if (!item) return
      var margin = Style.space(6)
      var point = item.mapToItem(panelFlick.contentItem, 0, 0)
      var top = point.y
      var bottom = top + item.height
      var viewTop = panelFlick.contentY
      var viewBottom = viewTop + panelFlick.height
      var maxY = Math.max(0, panelFlick.contentHeight - panelFlick.height)
      if (top < viewTop + margin) panelFlick.contentY = Math.max(0, top - margin)
      else if (bottom > viewBottom - margin) panelFlick.contentY = Math.min(maxY, bottom + margin - panelFlick.height)
    })
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  // Every open re-reads the install state, the service state and the
  // settings; the cursor stays hidden until hover or the first key.
  onOpenedChanged: {
    root.clearConfirmOpen = false
    if (!opened) return
    cursorActive = false
    focusSection = typesuggest.installed ? "header" : "install"
    selectedIndex = 0
    if (panelFlick) panelFlick.contentY = 0
    typesuggest.refresh(true)
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }
  onSectionsChanged: clampCursor()

  Service {
    id: typesuggest
    settings: root.settings
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.iconGlyph
    foreground: root.barIconColor
    tooltipText: root.barTooltip
    onPressed: function(buttonCode) {
      if (buttonCode === Qt.RightButton) typesuggest.toggleService()
      else root.toggle()
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(380))
    contentHeight: panel.fittedContentHeight(column.implicitHeight, Style.space(640))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onMoveRequested: function(dx, dy) {
        if (root.clearConfirmOpen) {
          if (dx !== 0) root.toggleConfirmChoice()
          return
        }
        if (!root.cursorActive) {
          root.cursorActive = true
          root.clampCursor()
          return
        }
        root.moveCursor(dx, dy)
      }
      onActivateRequested: {
        if (root.clearConfirmOpen) {
          if (clearConfirm.selectedIndex === 1) root.confirmClear()
          else root.cancelClear()
          return
        }
        if (root.cursorActive) root.activateCursor()
      }
      onCloseRequested: {
        if (root.clearConfirmOpen) root.cancelClear()
        else root.close()
      }
      onTabRequested: function(direction) {
        if (root.clearConfirmOpen) root.toggleConfirmChoice()
        else root.switchPanel(direction)
      }
      onTextKey: function(t) {
        if (root.clearConfirmOpen) return
        if (t === "t" || t === "T") typesuggest.toggleService()
        else if (t === "r" || t === "R") typesuggest.refresh(true)
      }

      Flickable {
        id: panelFlick
        anchors.fill: parent
        contentWidth: width
        contentHeight: column.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        flickableDirection: Flickable.VerticalFlick
        interactive: contentHeight > height
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

        Column {
          id: column
          // A 1px inset keeps the outer borders of the first and last buttons inside the clip
          x: 1
          width: panelFlick.width - 2
          spacing: Style.space(12)

          // ---------- Hero: keyboard icon · title/status · on/off ----------
          Item {
            id: header
            width: parent.width
            implicitHeight: hero.implicitHeight
            // Reached from the hero's trailingControl, like the Tailscale panel.
            readonly property bool ringVisible: root.cursorAt("header", 0)
            function focusHero() { root.setCursor("header", 0) }

            PanelHero {
              id: hero
              width: parent.width
              title: "TypeSuggest"
              meta: root.heroMeta
              foreground: root.foreground
              fontFamily: root.fontFamily
              iconOpacity: typesuggest.active ? 1.0 : 0.5
              // Status only; the switch owns turning it on and off.
              iconComponent: Component {
                Text {
                  textFormat: Text.PlainText
                  text: root.iconGlyph
                  color: root.iconColor
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.display
                }
              }

              trailingControl: Component {
                ToggleSwitch {
                  id: powerSwitch
                  visible: typesuggest.installed
                  checked: typesuggest.active
                  busy: typesuggest.serviceBusy
                  hasCursor: header.ringVisible
                  foreground: hero.foreground
                  onHovered: function(on) { if (on) header.focusHero() }
                  onToggled: typesuggest.toggleService()

                  PanelToolTip {
                    visible: powerSwitch.containsMouse
                    text: root.toggleHint
                    fontFamily: hero.fontFamily
                  }
                }
              }
            }
          }

          Text {
            textFormat: Text.PlainText
            visible: root.statusText !== ""
            width: parent.width
            text: root.statusText
            color: root.statusIsError ? root.urgent : root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
            wrapMode: Text.WordWrap
          }

          // ---------- Not installed ----------
          Text {
            textFormat: Text.PlainText
            visible: typesuggest.probed && !typesuggest.installed
            width: parent.width
            text: "TypeSuggest shows Windows-style word suggestions at the text cursor while you type. It is not installed yet."
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.body
            wrapMode: Text.WordWrap
          }

          ActionRow {
            visible: typesuggest.probed && !typesuggest.installed
            width: parent.width
            section: "install"
            glyph: "󰇚"
            title: "Install TypeSuggest"
            caption: "Opens a terminal that lists each step and asks first"
            onActivated: root.installTypeSuggest()
          }

          // ---------- Installed, but too old for --config-json / --set ----------
          Text {
            textFormat: Text.PlainText
            visible: root.outdated
            width: parent.width
            text: "This TypeSuggest version cannot be configured from the panel. Update the typesuggest package, or edit the config file."
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
            wrapMode: Text.WordWrap
          }

          // ---------- Suggestion bar ----------
          PanelSeparator {
            visible: root.settingsReady
            foreground: root.foreground
          }

          Row {
            visible: root.settingsReady
            width: parent.width
            spacing: Style.space(12)

            ChoiceGroup {
              width: (parent.width - parent.spacing) / 2
              title: "POSITION"
              section: "look"
              baseIndex: 0
              options: [
                { value: "below", label: "Below" },
                { value: "above", label: "Above" }
              ]
              activeValues: [root.config.bar_position]
              onChosen: function(value) { typesuggest.setOption("bar_position", value) }
            }

            ChoiceGroup {
              width: (parent.width - parent.spacing) / 2
              title: "THEME"
              section: "look"
              baseIndex: 2
              options: [
                { value: "omarchy", label: "Omarchy" },
                { value: "default", label: "Default" }
              ]
              activeValues: [root.config.theme]
              onChosen: function(value) { typesuggest.setOption("theme", value) }
            }
          }

          Column {
            visible: root.settingsReady
            width: parent.width
            spacing: Style.space(6)

            Item {
              width: parent.width
              implicitHeight: Math.max(sizeHeader.implicitHeight, sizeValue.implicitHeight)

              PanelSectionHeader {
                id: sizeHeader
                text: "BAR SIZE"
                foreground: root.foreground
                fontFamily: root.fontFamily
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
              }

              // The exact value, which can sit between notches when it was
              // set in the config file.
              Text {
                id: sizeValue
                textFormat: Text.PlainText
                text: Model.formatScale(sizeSlider.dragging
                  ? root.scaleStops[Math.round(sizeSlider.liveValue)]
                  : root.config.bar_scale)
                color: Qt.darker(root.foreground, 1.4)
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
                font.bold: true
                anchors.right: parent.right
                anchors.rightMargin: Style.space(6)
                anchors.verticalCenter: parent.verticalCenter
              }
            }

            CursorSurface {
              id: sizeRow
              width: parent.width
              height: sizeSlider.implicitHeight + Style.spacing.controlGap
              hasCursor: root.cursorAt("size", 0)
              onHasCursorChanged: if (hasCursor) root.scrollItemIntoView(sizeRow)
              foreground: root.foreground
              outline: true

              PanelSlider {
                id: sizeSlider
                bar: root.bar
                anchors.fill: parent
                anchors.leftMargin: Style.space(6)
                anchors.rightMargin: Style.space(6)
                minimum: 0
                maximum: root.scaleStops.length - 1
                step: 1
                integer: true
                tickCount: root.scaleStops.length
                value: Model.nearestScaleIndex(root.config.bar_scale)
                onReleased: function(v) { root.setScale(root.scaleStops[Math.round(v)]) }
              }

              HoverHandler {
                onHoveredChanged: if (hovered) root.setCursor("size", 0)
              }
            }
          }

          ChoiceGroup {
            visible: root.settingsReady
            width: parent.width
            title: "SUGGESTIONS"
            section: "count"
            options: [
              { value: "1", label: "1" },
              { value: "2", label: "2" },
              { value: "3", label: "3" },
              { value: "4", label: "4" },
              { value: "5", label: "5" }
            ]
            activeValues: [String(root.config.max_candidates)]
            onChosen: function(value) { typesuggest.setOption("max_candidates", parseInt(value, 10)) }
          }

          ChoiceGroup {
            visible: root.settingsReady && root.selectKeySupported
            width: parent.width
            title: "SELECT KEY"
            section: "select"
            options: [
              { value: "up", label: "Up", icon: "󰁝" },
              { value: "down", label: "Down", icon: "󰁅" }
            ]
            activeValues: [root.config.select_key]
            onChosen: function(value) { typesuggest.setOption("select_key", value) }
          }

          ChoiceGroup {
            visible: root.settingsReady
            width: parent.width
            title: "ACCEPT KEYS"
            section: "keys"
            options: [
              { value: "enter", label: "Enter", icon: "󰌑" },
              { value: "space", label: "Space", icon: "󱁐" },
              { value: "tab", label: "Tab", icon: "󰌒" }
            ]
            activeValues: root.config.accept_keys
            onChosen: function(value) { typesuggest.toggleAcceptKey(value) }
          }

          // How to take a suggestion and how the two key rows fit together, since nothing on
          // the bar itself says so
          Text {
            visible: root.settingsReady
            width: parent.width
            textFormat: Text.PlainText
            // Clicking arrived with select_key, so an older typesuggest offers keys only
            text: (root.selectKeySupported ? "Click a suggestion to take it, or press " : "While suggestions show, press ")
              + root.selectKeyLabel
              + " to highlight one, Left/Right to choose, then an accept key. " + root.backKeyLabel
              + " or Esc goes back to the text."
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
            wrapMode: Text.WordWrap
          }

          // ---------- Behavior ----------
          PanelSeparator {
            visible: root.settingsReady
            foreground: root.foreground
          }

          Column {
            visible: root.settingsReady
            width: parent.width
            spacing: Style.space(6)

            SwitchRow {
              width: parent.width
              section: "learn"
              title: "Learning"
              caption: "Rank the words you pick higher next time"
              checked: root.config.learn
              onToggled: typesuggest.setOption("learn", !root.config.learn)
            }

            SwitchRow {
              width: parent.width
              section: "typo"
              title: "Typo correction"
              caption: "Suggest close words when nothing matches"
              checked: root.config.typo_correction
              onToggled: typesuggest.setOption("typo_correction", !root.config.typo_correction)
            }
          }

          // ---------- Actions ----------
          PanelSeparator {
            visible: typesuggest.installed
            foreground: root.foreground
          }

          Column {
            visible: typesuggest.installed
            width: parent.width
            spacing: Style.space(6)

            ActionRow {
              width: parent.width
              section: "clear"
              glyph: "󰃢"
              title: "Clear learned phrases"
              caption: "Forget the word pairs TypeSuggest has learned"
              onActivated: root.openClearConfirm()
            }

            ActionRow {
              width: parent.width
              section: "config"
              glyph: "󱇧"
              title: "Open config file"
              caption: typesuggest.configDisplayPath
              onActivated: root.openConfigFile()
            }
          }

          Row {
            visible: typesuggest.installed
            width: parent.width
            spacing: Style.space(6)

            Text {
              id: noteGlyph
              textFormat: Text.PlainText
              text: "󰋽"
              color: root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.caption
            }

            Text {
              textFormat: Text.PlainText
              width: parent.width - noteGlyph.width - parent.spacing
              text: "Changes apply the next time a text field is focused."
              color: root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.caption
              wrapMode: Text.WordWrap
            }
          }
        }
      }

      ConfirmDialog {
        id: clearConfirm
        anchors.fill: parent
        z: 10
        opened: root.clearConfirmOpen
        message: "Forget every phrase TypeSuggest has learned?"
        confirmText: "Clear"
        background: Color.popups.background
        foreground: root.foreground
        fontFamily: root.fontFamily
        onCanceled: root.cancelClear()
        onConfirmed: root.confirmClear()
      }
    }
  }

  Timer {
    id: phraseTimer
    interval: 2800
    running: root.opened && typesuggest.active
    repeat: true
    onTriggered: phraseSwap.restart()
  }

  SequentialAnimation {
    id: phraseSwap
    PropertyAnimation {
      target: hero; property: "metaOpacity"
      to: 0.0; duration: 180; easing.type: Easing.OutQuad
    }
    ScriptAction {
      script: root.phraseIndex = (root.phraseIndex + 1) % root.activePhrases.length
    }
    PropertyAnimation {
      target: hero; property: "metaOpacity"
      to: 1.0; duration: 260; easing.type: Easing.InQuad
    }
  }

  // Section header plus a row of equal-width preset buttons. Single-choice
  // groups pass the current value in activeValues; the accept keys pass
  // every key that is on.
  component ChoiceGroup: Column {
    id: group

    property string title: ""
    property string section: ""
    property int baseIndex: 0
    property var options: []
    property var activeValues: []
    readonly property bool cursorInside: root.cursorActive && root.focusSection === group.section

    signal chosen(string value)

    spacing: Style.space(8)
    onCursorInsideChanged: if (group.cursorInside) root.scrollItemIntoView(group)

    PanelSectionHeader {
      text: group.title
      foreground: root.foreground
      fontFamily: root.fontFamily
    }

    Row {
      id: optionRow
      width: parent.width
      spacing: Style.space(6)

      readonly property real cellWidth: group.options.length > 0
        ? (width - spacing * (group.options.length - 1)) / group.options.length
        : 0

      Repeater {
        model: group.options

        Button {
          required property var modelData
          required property int index

          width: optionRow.cellWidth
          text: String(modelData.label)
          iconText: modelData.icon ? String(modelData.icon) : ""
          iconSize: Style.font.body
          fontSize: Style.font.bodySmall
          foreground: root.foreground
          fontFamily: root.fontFamily
          horizontalPadding: Style.spacing.sm
          verticalPadding: Style.spacing.controlPaddingY
          bordered: true
          active: group.activeValues.indexOf(String(modelData.value)) !== -1
          hasCursor: root.cursorAt(group.section, group.baseIndex + index)
          onClicked: group.chosen(String(modelData.value))
          onHovered: function(isHovered) {
            if (isHovered) root.setCursor(group.section, group.baseIndex + index)
          }
        }
      }
    }
  }

  // Title and caption on the left, a presentation-only switch on the right;
  // the whole row takes the click, like the shell's Toggle.
  component SwitchRow: CursorSurface {
    id: switchRow

    property string section: ""
    property string title: ""
    property string caption: ""
    property bool checked: false

    signal toggled()

    hasCursor: root.cursorAt(switchRow.section, 0)
    onHasCursorChanged: if (switchRow.hasCursor) root.scrollItemIntoView(switchRow)
    foreground: root.foreground
    implicitHeight: switchContent.implicitHeight + Style.spacing.rowPaddingX

    RowLayout {
      id: switchContent
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      anchors.leftMargin: Style.space(10)
      anchors.rightMargin: Style.space(8)
      spacing: Style.space(8)

      ColumnLayout {
        Layout.fillWidth: true
        spacing: Style.space(1)

        Text {
          textFormat: Text.PlainText
          Layout.fillWidth: true
          text: switchRow.title
          color: root.foreground
          font.family: root.fontFamily
          font.pixelSize: Style.font.body
          elide: Text.ElideRight
        }

        Text {
          textFormat: Text.PlainText
          Layout.fillWidth: true
          text: switchRow.caption
          color: root.dim
          font.family: root.fontFamily
          font.pixelSize: Style.font.caption
          elide: Text.ElideRight
        }
      }

      ToggleSwitch {
        checked: switchRow.checked
        interactive: false
        foreground: root.foreground
        Layout.alignment: Qt.AlignVCenter
      }
    }

    MouseArea {
      anchors.fill: parent
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onEntered: root.setCursor(switchRow.section, 0)
      onClicked: switchRow.toggled()
    }
  }

  // Icon, title and caption; activates on click or Enter.
  component ActionRow: CursorSurface {
    id: actionRow

    property string section: ""
    property string glyph: ""
    property string title: ""
    property string caption: ""

    signal activated()

    hasCursor: root.cursorAt(actionRow.section, 0)
    onHasCursorChanged: if (actionRow.hasCursor) root.scrollItemIntoView(actionRow)
    foreground: root.foreground
    implicitHeight: actionContent.implicitHeight + Style.spacing.rowPaddingX

    RowLayout {
      id: actionContent
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      anchors.leftMargin: Style.space(10)
      anchors.rightMargin: Style.space(10)
      spacing: Style.space(10)

      Text {
        textFormat: Text.PlainText
        text: actionRow.glyph
        color: root.foreground
        font.family: root.fontFamily
        font.pixelSize: Style.font.heading
        Layout.alignment: Qt.AlignVCenter
      }

      ColumnLayout {
        Layout.fillWidth: true
        spacing: Style.space(1)

        Text {
          textFormat: Text.PlainText
          Layout.fillWidth: true
          text: actionRow.title
          color: root.foreground
          font.family: root.fontFamily
          font.pixelSize: Style.font.body
          elide: Text.ElideRight
        }

        Text {
          textFormat: Text.PlainText
          Layout.fillWidth: true
          text: actionRow.caption
          visible: text !== ""
          color: root.dim
          font.family: root.fontFamily
          font.pixelSize: Style.font.caption
          elide: Text.ElideRight
        }
      }
    }

    MouseArea {
      anchors.fill: parent
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onEntered: root.setCursor(actionRow.section, 0)
      onClicked: actionRow.activated()
    }
  }
}
