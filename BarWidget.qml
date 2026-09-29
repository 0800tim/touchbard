import QtQuick
import Quickshell
import Quickshell.Io
import qs.Ui

// Omarchy bar icon for touchbard. Shows whether the Touch Bar daemon runs;
// left-click opens its config (or the installer when it isn't installed),
// right-click toggles the equaliser, middle-click restarts the session agent.
BarWidget {
  id: root
  moduleName: "io.github.0800tim.touchbard"

  property string status: "unknown"   // active | inactive | missing
  readonly property string pluginDir: Qt.resolvedUrl(".").toString().replace("file://", "")

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  Process {
    id: probe
    running: false
    command: ["sh", "-c", "command -v touchbard >/dev/null || { echo missing; exit; }; systemctl is-active touchbard 2>/dev/null || true"]
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: root.status = text.trim() === "" ? "inactive" : text.trim()
    }
  }

  Timer {
    interval: 15000
    running: true
    repeat: true
    triggeredOnStart: true
    onTriggered: probe.running = true
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: "\u{F030C}"
    dimmed: root.status !== "active"
    tooltipText: root.status === "missing"
      ? "Touch Bar: not installed. Click to install"
      : root.status === "active"
        ? "Touch Bar: running\nLeft: settings · Right: equaliser on/off · Middle: restart"
        : "Touch Bar daemon not running. Click for settings, middle-click to restart"

    onPressed: function(b) {
      if (!root.bar) return
      if (root.status === "missing") {
        root.bar.run("xdg-terminal-exec --app-id=touchbard-install -- bash '" + root.pluginDir + "install-touchbard'")
      } else if (b === Qt.RightButton) {
        root.bar.run("touchbar-agent eq toggle")
      } else if (b === Qt.MiddleButton) {
        root.bar.run("systemctl --user restart touchbar-agent")
        probe.running = true
      } else {
        root.bar.run("omarchy-launch-editor \"$HOME/.config/touchbar/config.toml\"")
      }
    }
  }
}
