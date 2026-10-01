import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import "Model.js" as Model

// State and commands behind the TypeSuggest widget. The panel reads the
// properties below and calls the functions; nothing here touches the UI.
//
// Every command is an argv array. The only values that reach one are the
// fixed choices from Model.js and paths this plugin builds itself, so no
// shell ever re-parses user or config data.
Item {
  id: root

  property var settings: ({})

  // --- probe results -------------------------------------------------------

  property bool probed: false
  property bool installed: false
  // The installed build has --config-json and --set (checked via --help).
  property bool capabilityChecked: false
  property bool supportsSettings: false
  property bool running: false

  // Optimistic on/off so the switch moves the instant it is clicked, the way
  // the Tailscale service does it: -1 follows the real state, 0/1 holds the
  // requested state until a verify probe has read the outcome.
  property int _desired: -1
  readonly property bool active: _desired === -1 ? running : (_desired === 1)

  property var config: Model.defaultConfig()
  property bool configLoaded: false

  property string actionStatus: ""
  // Errors from the last command, and from reading the settings, kept apart
  // so a successful read never hides a failed command (or the reverse).
  property string lastError: ""
  property string configError: ""
  readonly property string startFailedText: "TypeSuggest stopped right after starting. Only one input method can run at a time; is Fcitx5 still on?"

  readonly property string home: Quickshell.env("HOME")
  readonly property string configPath: Model.configPath(Quickshell.env("XDG_CONFIG_HOME"), home)
  readonly property string configDisplayPath: Model.displayPath(configPath, home)
  readonly property int refreshIntervalSec: intSetting("refreshIntervalSec", 30, 5, 3600)

  // Commands run one at a time from this queue, so a burst of clicks lands in
  // order and the last choice wins. Queued `--set`s for the same key coalesce.
  property var _queue: []
  property var _current: null
  readonly property bool busy: actionProcess.running || _queue.length > 0
  readonly property bool serviceBusy: hasQueued("service") || (_current !== null && _current.group === "service")

  property bool _wantConfig: false
  property bool _pendingConfigReload: false
  property bool _settleDesired: false
  property bool _verifyStart: false

  function setting(name, fallback) {
    var value = settings ? settings[name] : undefined
    return value === undefined || value === null ? fallback : value
  }

  function intSetting(name, fallback, min, max) {
    var n = parseInt(String(setting(name, fallback)), 10)
    if (!isFinite(n)) n = fallback
    return Math.max(min, Math.min(max, n))
  }

  // --- reading state -------------------------------------------------------

  // Cheap status check for the bar icon; `withConfig` also re-reads the
  // settings, which the panel asks for each time it opens.
  function refresh(withConfig) {
    if (withConfig === true) _wantConfig = true
    if (!installProbe.running) installProbe.running = true
    if (!probeWatchdog.running) probeWatchdog.start()
  }

  function loadConfig() {
    if (!installed || !supportsSettings) return
    // A queued --set would be undone by reading the file before it lands;
    // read once the queue drains instead.
    if (busy) {
      _pendingConfigReload = true
      return
    }
    if (configProcess.running) return
    configProcess.running = true
    if (!probeWatchdog.running) probeWatchdog.start()
  }

  function applyInstalled(isInstalled) {
    probed = true
    installed = isInstalled
    if (!isInstalled) {
      running = false
      _desired = -1
      capabilityChecked = false
      supportsSettings = false
      configLoaded = false
      configError = ""
      _wantConfig = false
      return
    }
    if (!stateProbe.running) stateProbe.running = true
    // Ask again on each panel open while the build looked too old, so an
    // upgrade shows the settings without restarting the shell.
    if (!capabilityChecked || (_wantConfig && !supportsSettings)) {
      if (!helpProbe.running) helpProbe.running = true
    } else if (_wantConfig) {
      _wantConfig = false
      loadConfig()
    }
  }

  function applyCapability(helpText) {
    capabilityChecked = true
    supportsSettings = Model.helpSupportsSettings(helpText)
    if (!supportsSettings) configLoaded = false
    if (_wantConfig) {
      _wantConfig = false
      loadConfig()
    }
  }

  function applyState(stdout) {
    running = String(stdout || "").trim() === "active"
    if (running && lastError === startFailedText) lastError = ""
    if (_settleDesired) {
      _settleDesired = false
      _desired = -1
    } else if (_desired !== -1 && running === (_desired === 1)) {
      _desired = -1
    }
    if (_verifyStart) {
      _verifyStart = false
      if (!running) lastError = startFailedText
    }
  }

  function applyConfig(exitCode, stdout, stderr) {
    if (exitCode !== 0) {
      configLoaded = false
      configError = Model.errorText(stderr, stdout, "Could not read the TypeSuggest settings")
      return
    }
    var parsed = Model.parseConfig(stdout)
    if (!parsed.ok) {
      configLoaded = false
      configError = parsed.error
      return
    }
    config = parsed.config
    configLoaded = true
    configError = ""
  }

  // --- changing state ------------------------------------------------------

  // Write one setting. The value is shown at once and written in the
  // background; a refused value is reported and the file is read back.
  function setOption(key, value) {
    if (!installed || !supportsSettings || !configLoaded) return
    var patch = {}
    patch[key] = value
    config = Object.assign({}, config, patch)
    lastError = ""
    enqueue({
      group: "set:" + key,
      kind: "set",
      key: key,
      argv: ["typesuggest", "--set", key, Model.settingArg(value)],
      failText: "Could not change " + key
    })
  }

  // Returns false (and changes nothing) when it would turn off the last key.
  function toggleAcceptKey(key) {
    var result = Model.toggleAcceptKey(config.accept_keys, key)
    if (!result.ok) {
      showStatus("At least one key has to accept a suggestion")
      return false
    }
    setOption("accept_keys", result.keys)
    return true
  }

  // On/off also sets autostart so the switch means the same after a login.
  function turnOn() {
    if (!installed || serviceBusy) return
    _desired = 1
    lastError = ""
    enqueue({ group: "service", kind: "service", argv: ["typesuggest", "--enable-autostart"], failText: "Could not enable autostart" })
    enqueue({ group: "service", kind: "service", argv: ["systemctl", "--user", "start", "typesuggest"], verifyStart: true, failText: "Could not start TypeSuggest" })
  }

  function turnOff() {
    if (!installed || serviceBusy) return
    _desired = 0
    lastError = ""
    enqueue({ group: "service", kind: "service", argv: ["systemctl", "--user", "stop", "typesuggest"], failText: "Could not stop TypeSuggest" })
    enqueue({ group: "service", kind: "service", argv: ["typesuggest", "--disable-autostart"], failText: "Could not disable autostart" })
  }

  function toggleService() {
    if (active) turnOff()
    else turnOn()
  }

  // typesuggest restarts the service itself so the daemon forgets them too.
  function clearLearned() {
    if (!installed || hasQueued("clear")) return
    lastError = ""
    enqueue({ group: "clear", kind: "clear", argv: ["typesuggest", "--clear-learned"], okStatus: "Learned phrases cleared", failText: "Could not clear learned phrases" })
  }

  // Omarchy's own helper: a toast, then the editor from Omarchy's defaults.
  function openConfig() {
    Util.execArgv(["omarchy-launch-config-editor", configPath])
  }

  // Opens a floating terminal that explains every step and asks before doing
  // anything. The script ships with this plugin; its path is quoted because
  // the Omarchy launcher takes one command string.
  function install(scriptPath) {
    if (!scriptPath) return
    Util.execArgv(["omarchy-launch-floating-terminal-with-presentation", "bash " + Util.shellQuote(scriptPath)])
  }

  function showStatus(text) {
    actionStatus = text
    actionStatusTimer.restart()
  }

  // --- command queue -------------------------------------------------------

  function hasQueued(group) {
    for (var i = 0; i < _queue.length; i++)
      if (_queue[i].group === group) return true
    return false
  }

  function enqueue(step) {
    var next = _queue.slice()
    if (step.kind === "set") {
      for (var i = 0; i < next.length; i++) {
        if (next[i].kind === "set" && next[i].key === step.key) {
          next[i] = step
          _queue = next
          return
        }
      }
    }
    next.push(step)
    _queue = next
    Qt.callLater(root.pump)
  }

  function pump() {
    if (actionProcess.running || _queue.length === 0) return
    var step = _queue[0]
    _queue = _queue.slice(1)
    _current = step
    actionProcess.exitSeen = false
    actionProcess.command = step.argv
    actionProcess.running = true
    actionWatchdog.restart()
  }

  function settleService(verifyStart) {
    _verifyStart = verifyStart
    settleTimer.interval = verifyStart ? 2000 : 600
    settleTimer.restart()
  }

  function finishStep(exitCode, stdout, stderr) {
    actionWatchdog.stop()
    var step = _current
    _current = null
    if (step) {
      if (exitCode !== 0) {
        lastError = Model.errorText(stderr, stdout, step.failText || "TypeSuggest command failed")
        // Drop the rest of the group: no point starting a service whose
        // autostart could not be set up.
        _queue = _queue.filter(function(s) { return s.group !== step.group })
        if (step.kind === "service") settleService(false)
        if (step.kind === "set") _pendingConfigReload = true
      } else {
        if (step.okStatus) showStatus(step.okStatus)
        if (step.kind === "set") _pendingConfigReload = true
        if (step.kind === "service" && !hasQueued("service")) settleService(step.verifyStart === true)
      }
    }
    if (_queue.length > 0) {
      Qt.callLater(root.pump)
    } else if (_pendingConfigReload) {
      _pendingConfigReload = false
      Qt.callLater(root.loadConfig)
    }
  }

  // --- timers --------------------------------------------------------------

  // Keeps the bar icon honest when the service is switched elsewhere (a
  // keybinding, the CLI). Only the cheap status probe runs here.
  Timer {
    interval: root.refreshIntervalSec * 1000
    repeat: true
    running: root.visible
    triggeredOnStart: true
    onTriggered: root.refresh(false)
  }

  // After a start, wait long enough to see a daemon that exits at once (it
  // does when another input method holds the seat), then read the result.
  Timer {
    id: settleTimer
    repeat: false
    onTriggered: {
      root._settleDesired = true
      root.refresh(false)
    }
  }

  Timer {
    id: actionStatusTimer
    interval: 2200
    repeat: false
    onTriggered: root.actionStatus = ""
  }

  // A probe that never exits would block every later refresh, since a running
  // Process cannot be restarted. Reap stragglers so the next tick starts clean.
  Timer {
    id: probeWatchdog
    interval: 10000
    repeat: false
    onTriggered: {
      if (installProbe.running) installProbe.running = false
      if (helpProbe.running) helpProbe.running = false
      if (stateProbe.running) stateProbe.running = false
      if (configProcess.running) configProcess.running = false
    }
  }

  Timer {
    id: actionWatchdog
    interval: 30000
    repeat: false
    onTriggered: if (actionProcess.running) actionProcess.running = false
  }

  // --- processes -----------------------------------------------------------

  // Quickshell ends both output streams before it emits exited, so reading
  // the collectors in onExited sees the complete output (the Tailscale
  // service relies on the same order).

  Process {
    id: installProbe
    command: ["sh", "-c", "command -v typesuggest"]
    stdout: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) { root.applyInstalled(exitCode === 0) }
  }

  Process {
    id: helpProbe
    command: ["typesuggest", "--help"]
    stdout: StdioCollector { id: helpStdout; waitForEnd: true }
    onExited: function(exitCode) { root.applyCapability(exitCode === 0 ? helpStdout.text : "") }
  }

  Process {
    id: stateProbe
    command: ["systemctl", "--user", "is-active", "typesuggest"]
    stdout: StdioCollector { id: stateStdout; waitForEnd: true }
    onExited: function(exitCode) { root.applyState(stateStdout.text) }
  }

  Process {
    id: configProcess
    command: ["typesuggest", "--config-json"]
    stdout: StdioCollector { id: configStdout; waitForEnd: true }
    stderr: StdioCollector { id: configStderr; waitForEnd: true }
    onExited: function(exitCode) { root.applyConfig(exitCode, configStdout.text, configStderr.text) }
  }

  Process {
    id: actionProcess
    command: []
    property bool exitSeen: false
    stdout: StdioCollector { id: actionStdout; waitForEnd: true }
    stderr: StdioCollector { id: actionStderr; waitForEnd: true }
    onExited: function(exitCode) {
      exitSeen = true
      root.finishStep(exitCode, actionStdout.text, actionStderr.text)
    }
    // A command that cannot start at all (binary gone) never emits exited;
    // fail the step so the queue does not wait on it forever.
    onRunningChanged: {
      if (running) return
      if (!exitSeen && root._current !== null) root.finishStep(-1, "", "")
      exitSeen = false
    }
  }
}
