import ApplicationServices
import Cocoa
import Darwin
import Foundation

// Which windows are Apple's sign-in prompts is read from their structure, not
// their words: a dialog (the accessibility subrole macOS gives an alert)
// owned by an application in Apple's bundle namespace. The verification-code
// dialog is the one that shows exactly one group of digits; the trusted-device
// prompt is the one with a default and a cancel button and no digits, and its
// default button is the one pressed. Lists of agent process names, window-title
// words, window sizes and prompt sentences decided this before, and stopped
// matching whenever macOS renamed an agent or reworded a prompt, or the session
// was not in English.

let helperVersion = "dialog-structure"
let arguments = CommandLine.arguments
if arguments.contains("--version") {
  print("stado-apple-challenge-capture \(helperVersion)")
  exit(EXIT_SUCCESS)
}

/// The bundle namespace of Apple's own applications and agents.
let appleBundlePrefix = "com.apple."

func argumentValue(_ name: String) -> String? {
  guard let index = arguments.firstIndex(of: name) else { return nil }
  let next = arguments.index(after: index)
  return arguments.indices.contains(next) ? arguments[next] : nil
}

let home = FileManager.default.homeDirectoryForCurrentUser.standardizedFileURL
let preflightOnly = arguments.contains("--preflight")
func validatedOutputFile() -> String {
  guard let outputArgument = argumentValue("--output-file") else {
    fputs("--output-file is required\n", stderr)
    exit(EXIT_FAILURE)
  }
  let outputURL = URL(fileURLWithPath: outputArgument).standardizedFileURL
  let workRoot = home.appendingPathComponent(".stado/work", isDirectory: true)
    .resolvingSymlinksInPath().path + "/"
  let outputParent = outputURL.deletingLastPathComponent().resolvingSymlinksInPath().path + "/"
  guard outputURL.path.hasPrefix(workRoot), outputParent.hasPrefix(workRoot) else {
    fputs("--output-file must stay under ~/.stado/work\n", stderr)
    exit(EXIT_FAILURE)
  }
  return outputURL.path
}
let outputFile = preflightOnly ? "" : validatedOutputFile()
let clickAllow = arguments.contains("--click-allow")
let clickDone = arguments.contains("--click-done")

struct Target {
  let pid: pid_t
  let name: String
  let bundle: String
}

/// One dialog on screen: its owner, its element, its text and its buttons.
struct Dialog {
  let target: Target
  let window: AXUIElement
  let text: String
  let defaultButton: AXUIElement?
  let cancelButton: AXUIElement?
}

enum CaptureFailure: Error {
  case message(String)
}

func jsonPrint(_ value: [String: Any]) {
  let data = try! JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys])
  print(String(data: data, encoding: .utf8)!)
}

/// An attribute's text, or nothing when the element does not carry it.
func axString(_ element: AXUIElement, _ attr: CFString) -> String? {
  var raw: CFTypeRef?
  guard AXUIElementCopyAttributeValue(element, attr, &raw) == .success, let raw else { return nil }
  if let value = raw as? String { return value }
  return String(describing: raw)
}

func axElement(_ element: AXUIElement, _ attr: CFString) -> AXUIElement? {
  var raw: CFTypeRef?
  guard AXUIElementCopyAttributeValue(element, attr, &raw) == .success, let raw,
        CFGetTypeID(raw) == AXUIElementGetTypeID() else { return nil }
  return (raw as! AXUIElement)
}

/// The elements an attribute lists; an element without the attribute lists none.
func axChildren(_ element: AXUIElement, _ attr: CFString) -> [AXUIElement] {
  var raw: CFTypeRef?
  guard AXUIElementCopyAttributeValue(element, attr, &raw) == .success,
        let children = raw as? [AXUIElement] else { return [] }
  return children
}

/// Every string the element and its descendants show, depth first. `seen`
/// guards against a tree that refers back to itself.
func shownText(_ element: AXUIElement, seen: inout [AXUIElement]) -> [String] {
  if seen.contains(where: { CFEqual($0, element) }) { return [] }
  seen.append(element)
  var shown = [
    kAXTitleAttribute, kAXValueAttribute, kAXDescriptionAttribute, kAXHelpAttribute,
  ].compactMap { axString(element, $0 as CFString) }.filter {
    !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }
  for attr in [kAXChildrenAttribute, kAXVisibleChildrenAttribute, kAXContentsAttribute] {
    for child in axChildren(element, attr as CFString) {
      shown.append(contentsOf: shownText(child, seen: &seen))
    }
  }
  return shown
}

/// The running applications in Apple's bundle namespace.
func appleTargets() -> [Target] {
  NSWorkspace.shared.runningApplications.compactMap { app in
    guard let bundle = app.bundleIdentifier, bundle.hasPrefix(appleBundlePrefix) else { return nil }
    return Target(pid: app.processIdentifier, name: app.localizedName ?? bundle, bundle: bundle)
  }
}

/// Whether a window is an alert: the dialog subroles macOS gives one.
func isDialog(_ window: AXUIElement) -> Bool {
  let subrole = axString(window, kAXSubroleAttribute as CFString)
  return subrole == (kAXDialogSubrole as String) || subrole == (kAXSystemDialogSubrole as String)
}

/// Every dialog Apple's applications show right now.
func appleDialogs() -> [Dialog] {
  appleTargets().flatMap { target -> [Dialog] in
    let application = AXUIElementCreateApplication(target.pid)
    return axChildren(application, kAXWindowsAttribute as CFString).filter(isDialog).map { window in
      var seen: [AXUIElement] = []
      return Dialog(
        target: target,
        window: window,
        text: shownText(window, seen: &seen).joined(separator: "\n"),
        defaultButton: axElement(window, kAXDefaultButtonAttribute as CFString),
        cancelButton: axElement(window, kAXCancelButtonAttribute as CFString)
      )
    }
  }
}
