import ApplicationServices
import Cocoa
import CoreGraphics
import Darwin
import Foundation

let helperVersion = "2"
let arguments = CommandLine.arguments
if arguments.contains("--version") {
  print("stado-apple-challenge-capture \(helperVersion)")
  exit(0)
}

func argumentValue(_ name: String) -> String? {
  guard let index = arguments.firstIndex(of: name), arguments.indices.contains(index + 1) else {
    return nil
  }
  return arguments[index + 1]
}

let home = FileManager.default.homeDirectoryForCurrentUser.standardizedFileURL
let preflightOnly = arguments.contains("--preflight")
func validatedOutputFile() -> String {
  guard let outputArgument = argumentValue("--output-file") else {
    fputs("--output-file is required\n", stderr)
    exit(2)
  }
  let outputURL = URL(fileURLWithPath: outputArgument).standardizedFileURL
  let workRoot = home.appendingPathComponent(".stado/work", isDirectory: true)
    .resolvingSymlinksInPath().path + "/"
  let outputParent = outputURL.deletingLastPathComponent().resolvingSymlinksInPath().path + "/"
  guard outputURL.path.hasPrefix(workRoot), outputParent.hasPrefix(workRoot) else {
    fputs("--output-file must stay under ~/.stado/work\n", stderr)
    exit(2)
  }
  return outputURL.path
}
let outputFile = preflightOnly ? "" : validatedOutputFile()
let clickAllow = arguments.contains("--click-allow")
let clickDone = arguments.contains("--click-done")
let waitSeconds = min(max(Double(argumentValue("--wait-seconds") ?? "120") ?? 120, 1), 120)
let maxDepth = 12
let pidArgs: [pid_t] = arguments.enumerated().compactMap { index, value in
  guard value == "--pid", arguments.indices.contains(index + 1) else { return nil }
  return pid_t(Int32(arguments[index + 1]) ?? 0)
}.filter { $0 > 0 }

struct CapturedNode {
  let role: String
  let title: String
  let value: String
  let description: String
  let help: String
}

struct Target {
  let pid: pid_t
  let name: String
  let bundle: String
  let source: String
}

struct PromptSnapshot {
  let target: Target
  let root: AXUIElement
  let rows: [(AXUIElement, CapturedNode)]
  let text: String
}

enum CaptureFailure: Error {
  case message(String)
}

func jsonPrint(_ value: [String: Any]) {
  let data = try! JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys])
  print(String(data: data, encoding: .utf8)!)
}

func number(_ value: Any?) -> Double {
  if let n = value as? NSNumber { return n.doubleValue }
  if let d = value as? Double { return d }
  if let i = value as? Int { return Double(i) }
  return 0
}

func axString(_ element: AXUIElement, _ attr: CFString) -> String {
  var raw: CFTypeRef?
  let err = AXUIElementCopyAttributeValue(element, attr, &raw)
  if err != .success { return "" }
  if let value = raw as? String { return value }
  if let value = raw { return String(describing: value) }
  return ""
}

func axChildren(_ element: AXUIElement, _ attr: CFString) -> [AXUIElement] {
  var raw: CFTypeRef?
  let err = AXUIElementCopyAttributeValue(element, attr, &raw)
  if err != .success { return [] }
  return raw as? [AXUIElement] ?? []
}

func sameElement(_ lhs: AXUIElement, _ rhs: AXUIElement) -> Bool {
  CFEqual(lhs, rhs)
}

func capture(
  _ element: AXUIElement,
  depth: Int = 0,
  seen: inout [AXUIElement]
) -> [(AXUIElement, CapturedNode)] {
  if depth > maxDepth || seen.contains(where: { sameElement($0, element) }) { return [] }
  seen.append(element)

  let node = CapturedNode(
    role: axString(element, kAXRoleAttribute as CFString),
    title: axString(element, kAXTitleAttribute as CFString),
    value: axString(element, kAXValueAttribute as CFString),
    description: axString(element, kAXDescriptionAttribute as CFString),
    help: axString(element, kAXHelpAttribute as CFString)
  )
  var rows: [(AXUIElement, CapturedNode)] = [(element, node)]
  let childAttrs = [
    kAXChildrenAttribute as CFString,
    kAXVisibleChildrenAttribute as CFString,
    kAXWindowsAttribute as CFString,
    kAXContentsAttribute as CFString,
  ]
  for attr in childAttrs {
    for child in axChildren(element, attr) {
      rows.append(contentsOf: capture(child, depth: depth + 1, seen: &seen))
    }
  }
  return rows
}

func appInfo(pid: pid_t) -> (String, String) {
  if let app = NSWorkspace.shared.runningApplications.first(where: { $0.processIdentifier == pid }) {
    return (app.localizedName ?? "", app.bundleIdentifier ?? "")
  }
  return ("", "")
}

func windowServerTargets() -> [Target] {
  let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
  let raw = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? []
  var targets: [Target] = []

  for win in raw {
    let owner = win[kCGWindowOwnerName as String] as? String ?? ""
    let title = win[kCGWindowName as String] as? String ?? ""
    let pid = pid_t((win[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value ?? 0)
    let layer = (win[kCGWindowLayer as String] as? NSNumber)?.intValue ?? 0
    let bounds = win[kCGWindowBounds as String] as? [String: Any] ?? [:]
    let width = number(bounds["Width"])
    let height = number(bounds["Height"])
    let ownerLower = owner.lowercased()
    let titleLower = title.lowercased()

    if pid <= 0 { continue }

    let namedAppleWindow = ownerLower.contains("followup")
      || ownerLower.contains("authentication")
      || ownerLower.contains("securityagent")
      || ownerLower.contains("usernotificationcenter")
      || titleLower.contains("followup")
      || titleLower.contains("authentication")
      || titleLower.contains("apple account")
      || titleLower.contains("apple id")

    let privateFollowUpPrompt = owner.localizedCaseInsensitiveContains("[ App")
      && layer == 25
      && width >= 300
      && width <= 900
      && height >= 200
      && height <= 800

    if !namedAppleWindow && !privateFollowUpPrompt { continue }

    let info = appInfo(pid: pid)
    targets.append(Target(
      pid: pid,
      name: info.0.isEmpty ? owner : info.0,
      bundle: info.1,
      source: "windowserver layer=\(layer) owner=\(owner) title=\(title)"
    ))
  }

  return targets
}

func processTargets() -> [Target] {
  let names = [
    "AuthenticationServicesAgent",
    "CoreServicesUIAgent",
    "SecurityAgent",
    "AppleIDSettings",
    "System Settings",
    "UserNotificationCenter",
    "NotificationCenter",
    "FollowUpUI",
  ]
  if !pidArgs.isEmpty {
    return pidArgs.map { pid in
      let info = appInfo(pid: pid)
      return Target(pid: pid, name: info.0, bundle: info.1, source: "cli")
    }
  }

  let appTargets = NSWorkspace.shared.runningApplications.compactMap { app -> Target? in
    let name = app.localizedName ?? ""
    let bundle = app.bundleIdentifier ?? ""
    let matched = names.contains(name)
      || bundle.contains("AuthenticationServices")
      || bundle.contains("AppleID")
      || bundle.contains("UserNotification")
      || bundle.contains("FollowUp")
    if !matched { return nil }
    return Target(pid: app.processIdentifier, name: name, bundle: bundle, source: "nsworkspace")
  }

  var byPid: [pid_t: Target] = [:]
  for target in appTargets + windowServerTargets() {
    if let existing = byPid[target.pid] {
      byPid[target.pid] = Target(
        pid: target.pid,
        name: existing.name.isEmpty ? target.name : existing.name,
        bundle: existing.bundle.isEmpty ? target.bundle : existing.bundle,
        source: "\(existing.source),\(target.source)"
      )
    } else {
      byPid[target.pid] = target
    }
  }
  return byPid.values.sorted { $0.pid < $1.pid }
}

func normalizedLabel(_ node: CapturedNode) -> String {
  [node.title, node.value, node.description, node.help]
    .joined(separator: " ")
    .replacingOccurrences(of: "\\s+", with: " ", options: .regularExpression)
    .trimmingCharacters(in: .whitespacesAndNewlines)
}

func exactButtons(
  in rows: [(AXUIElement, CapturedNode)],
  labels: [String]
) -> [(AXUIElement, String)] {
  rows.compactMap { element, node in
    guard node.role == "AXButton" else { return nil }
    let label = normalizedLabel(node)
    guard labels.contains(where: { label.caseInsensitiveCompare($0) == .orderedSame }) else {
      return nil
    }
    return (element, label)
  }
}
