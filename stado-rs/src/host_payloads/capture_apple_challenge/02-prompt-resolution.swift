
/// The groups of digits a text shows. Digits separated only by spaces are one
/// group, because the code dialog may space the code for reading.
func digitGroups(_ text: String) -> Set<String> {
  var groups = Set<String>()
  var current = ""
  for character in text {
    if character.isASCII && character.isNumber {
      current.append(character)
    } else if character.isWhitespace && !character.isNewline && !current.isEmpty {
      continue
    } else {
      if !current.isEmpty { groups.insert(current) }
      current = ""
    }
  }
  if !current.isEmpty { groups.insert(current) }
  return groups
}

/// The one element of `values`, or none when there are none or several.
func onlyOne<T>(_ values: [T]) -> T? {
  guard let first = values.first, values.dropFirst().isEmpty else { return nil }
  return first
}

/// The trusted-device prompt: an Apple dialog asking to allow or refuse,
/// showing no digits.
func isAllowPrompt(_ dialog: Dialog) -> Bool {
  dialog.defaultButton != nil && dialog.cancelButton != nil && digitGroups(dialog.text).isEmpty
}

/// The verification-code dialog: an Apple dialog showing exactly one group of
/// digits, which is the code.
func shownCode(_ dialog: Dialog) -> String? {
  onlyOne(Array(digitGroups(dialog.text)))
}

// What a window said, with every digit replaced by `#`, so a failed capture
// tells "never shown" from "shown in a shape this helper does not read"
// without carrying the code itself.
func maskedPreview(_ text: String) -> String {
  String(text.map { $0.isNumber ? "#" : $0 })
    .replacingOccurrences(of: "\\s+", with: " ", options: .regularExpression)
    .trimmingCharacters(in: .whitespacesAndNewlines)
}

var dialogSummaries: [[String: Any]] = []

func press(_ button: AXUIElement?, in dialog: Dialog, actionName: String) throws {
  guard let button else {
    throw CaptureFailure.message("the \(actionName) dialog of \(dialog.target.bundle) has no default button")
  }
  guard AXUIElementPerformAction(button, kAXPressAction as CFString) == .success else {
    throw CaptureFailure.message("failed to press the default button of the \(actionName) dialog")
  }
}

/// Write the code where only this user can read it; a file already there is
/// refused, not overwritten. A file left behind by a failed write is named.
func writeOwnerOnlyCode(_ code: String, to path: String) throws {
  let url = URL(fileURLWithPath: path)
  do {
    try Data(code.utf8).write(to: url, options: [.withoutOverwriting])
    try FileManager.default.setAttributes(
      [.posixPermissions: NSNumber(value: S_IRUSR | S_IWUSR)], ofItemAtPath: path)
  } catch {
    var message = "the owner-only challenge file was not written: \(error.localizedDescription)"
    do {
      try FileManager.default.removeItem(at: url)
    } catch let removal {
      message += "; \(path) is left behind: \(removal.localizedDescription)"
    }
    throw CaptureFailure.message(message)
  }
}

let trusted = AXIsProcessTrusted()
if preflightOnly {
  jsonPrint([
    "version": helperVersion,
    "ok": trusted,
    "accessibilityTrusted": trusted,
  ])
  exit(trusted ? EXIT_SUCCESS : EXIT_FAILURE)
}
var clicked: [String] = []
var clickedAllow = false
var clickedDone = false
var doneFailure: String?
var code = ""
var errorMessage: String?
var verifiedPid: pid_t?
var captureFailure: Error?
var observedPids: Set<pid_t> = []
var observers: [AXObserver] = []

// One look at Apple's dialogs on screen. True when the run has its answer:
// the code, or a failure that names what was seen.
func examinePrompts() -> Bool {
  do {
    let dialogs = appleDialogs()
    dialogSummaries = dialogs.map { dialog in
      [
        "pid": dialog.target.pid,
        "name": dialog.target.name,
        "bundle": dialog.target.bundle,
        "hasDefaultButton": dialog.defaultButton != nil,
        "hasCancelButton": dialog.cancelButton != nil,
        "textPreview": maskedPreview(dialog.text),
      ]
    }

    if clickAllow && !clickedAllow {
      let allowPrompts = dialogs.filter(isAllowPrompt)
      if !allowPrompts.isEmpty {
        guard let allowPrompt = onlyOne(allowPrompts) else {
          throw CaptureFailure.message("more than one Apple allow-or-refuse dialog is visible")
        }
        try press(allowPrompt.defaultButton, in: allowPrompt, actionName: "allow")
        clicked.append("default button of \(allowPrompt.target.bundle)")
        clickedAllow = true
        verifiedPid = allowPrompt.target.pid
      }
    }

    let codePrompts = dialogs.compactMap { dialog in shownCode(dialog).map { (dialog, $0) } }
    if !codePrompts.isEmpty {
      guard let pair = onlyOne(codePrompts) else {
        throw CaptureFailure.message("more than one Apple dialog shows a code")
      }
      let (codePrompt, shown) = pair
      if let verifiedPid, codePrompt.target.pid != verifiedPid {
        throw CaptureFailure.message("the Apple code appeared in a different process than the allow dialog")
      }
      code = shown
      if clickDone {
        do {
          try press(codePrompt.defaultButton, in: codePrompt, actionName: "code")
          clicked.append("default button of \(codePrompt.target.bundle)")
          clickedDone = true
        } catch CaptureFailure.message(let message) {
          doneFailure = message
        }
      }
      return true
    }
    observePromptProcesses()
    return false
  } catch {
    captureFailure = error
    return true
  }
}

// Accessibility tells this helper when a watched process creates a window or
// changes what it shows; each notice is one more look at the dialogs.
let promptChanged: AXObserverCallback = { _, _, _, _ in
  if examinePrompts() { CFRunLoopStop(CFRunLoopGetMain()) }
}

func observePromptProcesses() {
  for target in appleTargets() where !observedPids.contains(target.pid) {
    var observer: AXObserver?
    guard AXObserverCreate(target.pid, promptChanged, &observer) == .success, let observer else {
      continue
    }
    let app = AXUIElementCreateApplication(target.pid)
    for notification in [
      kAXWindowCreatedNotification,
      kAXFocusedWindowChangedNotification,
      kAXFocusedUIElementChangedNotification,
      kAXValueChangedNotification,
      kAXTitleChangedNotification,
    ] {
      AXObserverAddNotification(observer, app, notification as CFString, nil)
    }
    CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(observer), .defaultMode)
    observers.append(observer)
    observedPids.insert(target.pid)
  }
}

if !trusted {
  errorMessage = "Accessibility permission is not granted"
} else {
  // No clock decides when to give up: the run ends when the code is read or a
  // dialog shows something this helper refuses. An agent that starts later
  // (Apple's prompt agents start on demand) is picked up on launch.
  if !examinePrompts() {
    let center = NSWorkspace.shared.notificationCenter
    for name in [NSWorkspace.didLaunchApplicationNotification, NSWorkspace.didActivateApplicationNotification] {
      center.addObserver(forName: name, object: nil, queue: .main) { _ in
        if examinePrompts() { CFRunLoopStop(CFRunLoopGetMain()) }
      }
    }
    CFRunLoopRun()
  }
  do {
    if let captureFailure { throw captureFailure }
    guard !code.isEmpty else {
      throw CaptureFailure.message("the run ended without an Apple verification code")
    }
    try writeOwnerOnlyCode(code, to: outputFile)
  } catch CaptureFailure.message(let message) {
    errorMessage = message
    code = ""
  } catch {
    errorMessage = "native Apple challenge capture failed closed: \(error.localizedDescription)"
    code = ""
  }
}

jsonPrint([
  "version": helperVersion,
  "ok": trusted && errorMessage == nil,
  "accessibilityTrusted": trusted,
  "clicked": clicked,
  "clickedAllow": clickedAllow,
  "clickedDone": clickedDone,
  "doneFailure": doneFailure ?? NSNull(),
  "codeCaptured": !code.isEmpty && errorMessage == nil,
  "outputFile": !code.isEmpty && errorMessage == nil ? outputFile : NSNull(),
  "error": errorMessage ?? NSNull(),
  "dialogs": dialogSummaries,
])
if errorMessage != nil {
  exit(EXIT_FAILURE)
}
