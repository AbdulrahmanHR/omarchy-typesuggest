// Pure helpers for the TypeSuggest panel: reading `typesuggest --config-json`,
// the fixed choices the panel offers, and the argument each choice becomes for
// `typesuggest --set`. No QML, process, or file access lives here.

function barScaleStops() {
  return [0.75, 1.0, 1.25, 1.5, 1.75, 2.0]
}

function acceptKeyNames() {
  return ["enter", "space", "tab"]
}

function positions() {
  return ["below", "above"]
}

function selectKeys() {
  return ["up", "down"]
}

function themes() {
  return ["omarchy", "default"]
}

function defaultConfig() {
  return {
    learn: true,
    min_prefix_length: 1,
    max_candidates: 3,
    bar_scale: 1.0,
    bar_position: "below",
    theme: "omarchy",
    select_key: "up",
    accept_keys: acceptKeyNames(),
    trailing_space: true,
    hover_highlight: true,
    mouse_hides_bar: false,
    typo_correction: true
  }
}

function clampInt(value, min, max, fallback) {
  var n = parseInt(String(value), 10)
  if (!isFinite(n)) return fallback
  return Math.max(min, Math.min(max, n))
}

function pick(value, allowed, fallback) {
  var text = String(value === undefined || value === null ? "" : value).toLowerCase()
  return allowed.indexOf(text) !== -1 ? text : fallback
}

// Known accept keys in the order the panel shows them. An empty or unknown
// list falls back to all three, which is what typesuggest itself does.
function normalizeAcceptKeys(list) {
  var given = Array.isArray(list) ? list.map(function(k) { return String(k).toLowerCase() }) : []
  var out = []
  var names = acceptKeyNames()
  for (var i = 0; i < names.length; i++) {
    var name = names[i]
    if (given.indexOf(name) !== -1 || (name === "enter" && given.indexOf("return") !== -1)) out.push(name)
  }
  return out.length > 0 ? out : names
}

function normalizeConfig(data) {
  var base = defaultConfig()
  var scale = Number(data.bar_scale)
  return {
    learn: typeof data.learn === "boolean" ? data.learn : base.learn,
    min_prefix_length: clampInt(data.min_prefix_length, 1, 10, base.min_prefix_length),
    max_candidates: clampInt(data.max_candidates, 1, 5, base.max_candidates),
    bar_scale: isFinite(scale) && scale > 0 ? scale : base.bar_scale,
    bar_position: pick(data.bar_position, positions(), base.bar_position),
    theme: pick(data.theme, themes(), base.theme),
    // null when the installed typesuggest predates select_key, so the panel
    // can leave out a setting that build would refuse
    select_key: data.select_key === undefined ? null : pick(data.select_key, selectKeys(), base.select_key),
    accept_keys: normalizeAcceptKeys(data.accept_keys),
    trailing_space: typeof data.trailing_space === "boolean" ? data.trailing_space : base.trailing_space,
    // null when the installed typesuggest predates these mouse settings (before 1.2.1),
    // so the panel leaves out rows that build would refuse
    hover_highlight: typeof data.hover_highlight === "boolean" ? data.hover_highlight : null,
    mouse_hides_bar: typeof data.mouse_hides_bar === "boolean" ? data.mouse_hides_bar : null,
    typo_correction: typeof data.typo_correction === "boolean" ? data.typo_correction : base.typo_correction
  }
}

// `typesuggest --config-json` prints one JSON object. Anything around it (a
// warning on the same stream) is ignored rather than failing the whole read.
function parseConfig(raw) {
  var text = String(raw || "").trim()
  var start = text.indexOf("{")
  var end = text.lastIndexOf("}")
  if (start === -1 || end < start) return { ok: false, error: "TypeSuggest printed no settings" }
  var data = null
  try {
    data = JSON.parse(text.substring(start, end + 1))
  } catch (e) {
    return { ok: false, error: "Could not read the TypeSuggest settings" }
  }
  if (data === null || typeof data !== "object" || Array.isArray(data))
    return { ok: false, error: "Could not read the TypeSuggest settings" }
  return { ok: true, config: normalizeConfig(data) }
}

// The panel only drives --config-json and --set, which older typesuggest
// builds do not have; those builds ignore unknown flags and would start a
// second daemon instead. `typesuggest --help` is safe on every version.
function helpSupportsSettings(helpText) {
  var text = String(helpText || "")
  return text.indexOf("--config-json") !== -1 && /--set\b/.test(text)
}

// The installed build's version from the first line of `typesuggest --help`
// ("typesuggest v1.2.0 - ..."), or "" when it does not say.
function helpVersion(helpText) {
  var match = /typesuggest v(\d+(?:\.\d+)*)/.exec(String(helpText || ""))
  return match ? match[1] : ""
}

// The plugin's version from its manifest.json text, or "" when unreadable.
function manifestVersion(text) {
  try {
    var data = JSON.parse(String(text || ""))
    return data && typeof data.version === "string" ? data.version : ""
  } catch (e) {
    return ""
  }
}

// "Plugin 1.2.0 · TypeSuggest 1.2.0", leaving out a version that is unknown.
function versionLine(pluginVersion, daemonVersion) {
  var parts = []
  if (pluginVersion) parts.push("Plugin " + pluginVersion)
  if (daemonVersion) parts.push("TypeSuggest " + daemonVersion)
  return parts.join(" · ")
}

// Accept keys after toggling `key`. Refuses to remove the last one, because a
// suggestion that no key can commit is useless.
function toggleAcceptKey(keys, key) {
  var current = normalizeAcceptKeys(keys)
  var has = current.indexOf(key) !== -1
  if (has && current.length === 1) return { ok: false, keys: current }
  var next = []
  var names = acceptKeyNames()
  for (var i = 0; i < names.length; i++) {
    var name = names[i]
    var on = name === key ? !has : current.indexOf(name) !== -1
    if (on) next.push(name)
  }
  return { ok: true, keys: next }
}

function nearestScaleIndex(scale) {
  var stops = barScaleStops()
  var value = Number(scale)
  var best = 0
  var bestDist = Infinity
  for (var i = 0; i < stops.length; i++) {
    var d = Math.abs(stops[i] - value)
    if (d < bestDist) {
      bestDist = d
      best = i
    }
  }
  return best
}

// Next notch up (delta > 0) or down from `scale`, which may sit between
// notches when it was set in the config file.
function stepScale(scale, delta) {
  var stops = barScaleStops()
  var value = Number(scale)
  var i
  if (delta > 0) {
    for (i = 0; i < stops.length; i++)
      if (stops[i] > value + 0.001) return stops[i]
    return stops[stops.length - 1]
  }
  for (i = stops.length - 1; i >= 0; i--)
    if (stops[i] < value - 0.001) return stops[i]
  return stops[0]
}

function scaleArg(scale) {
  return String(Math.round(Number(scale) * 100) / 100)
}

function formatScale(scale) {
  return scaleArg(scale) + "x"
}

// The value `typesuggest --set <key> <value>` expects for a panel setting.
function settingArg(value) {
  if (typeof value === "boolean") return value ? "true" : "false"
  if (Array.isArray(value)) return value.join(",")
  if (typeof value === "number") return scaleArg(value)
  return String(value)
}

// typesuggest reads $XDG_CONFIG_HOME when it is absolute, else ~/.config.
function configPath(xdgConfigHome, home) {
  var base = String(xdgConfigHome || "")
  if (base.charAt(0) !== "/") base = String(home || "") + "/.config"
  return base.replace(/\/+$/, "") + "/typesuggest/config.toml"
}

function displayPath(path, home) {
  var p = String(path || "")
  var h = String(home || "")
  if (h !== "" && p.indexOf(h + "/") === 0) return "~" + p.substring(h.length)
  return p
}

// Local filesystem path of a file:// URL from Qt.resolvedUrl().
function localPath(url) {
  var text = String(url || "")
  if (text.indexOf("file://") !== 0) return ""
  try {
    return decodeURIComponent(text.substring("file://".length))
  } catch (e) {
    return ""
  }
}

function errorText(stderr, stdout, fallback) {
  var text = String(stderr || "").trim() || String(stdout || "").trim() || fallback
  text = text.split("\n").filter(function(line) { return line.trim() !== "" }).pop() || fallback
  text = text.replace(/^typesuggest:\s*/, "").replace(/\s+/g, " ").trim()
  if (text === "") text = fallback
  // Capitalize a sentence, but leave a leading setting name such as
  // "bar_scale must be ..." as typesuggest spells it.
  if (!/^[a-z]+_/.test(text)) text = text.charAt(0).toUpperCase() + text.slice(1)
  return text.length > 140 ? text.substring(0, 137) + "…" : text
}
