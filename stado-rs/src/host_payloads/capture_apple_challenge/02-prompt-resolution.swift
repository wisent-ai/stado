
func promptText(_ rows: [(AXUIElement, CapturedNode)]) -> String {
  rows
    .flatMap { row in [row.1.title, row.1.value, row.1.description, row.1.help] }
    .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    .joined(separator: "\n")
}

func isAppleTrustedDeviceAllowPrompt(_ snapshot: PromptSnapshot) -> Bool {
  let lower = snapshot.text.lowercased()
  return lower.contains("apple")
    && (lower.contains("sign in") || lower.contains("sign-in"))
    && lower.contains("allow")
    && lower.contains("do not allow")
    && exactButtons(in: snapshot.rows, labels: ["Allow"]).count == 1
    && exactButtons(in: snapshot.rows, labels: ["Do Not Allow"]).count == 1
}

func isAppleVerificationCodePrompt(_ snapshot: PromptSnapshot) -> Bool {
  let lower = snapshot.text.lowercased()
  return lower.contains("apple") && lower.contains("verification") && lower.contains("code")
}

func sixDigitCodes(_ text: String) -> Set<String> {
  var candidates = Set<String>()
  var current = ""

  func finishCandidate() {
    if current.count == 6 { candidates.insert(current) }
    current = ""
  }

  for scalar in text.unicodeScalars {
    if CharacterSet.decimalDigits.contains(scalar) {
      current.append(Character(scalar))
    } else if (CharacterSet.whitespacesAndNewlines.contains(scalar) || scalar.value == 0x00a0)
      && !current.isEmpty
      && current.count < 6 {
      continue
    } else {
      finishCandidate()
    }
  }
  finishCandidate()
  return candidates
}

// What a window said, with every digit replaced by `#`. A failed capture used to
// report only "found none", which is indistinguishable from "the prompt was never
// shown" and from "the prompt was shown in a shape this helper does not match".
// The masked text tells those apart and cannot carry the code itself.
func maskedPreview(_ text: String) -> String {
  let masked = String(text.map { $0.isNumber ? "#" : $0 })
    .replacingOccurrences(of: "\\s+", with: " ", options: .regularExpression)
    .trimmingCharacters(in: .whitespacesAndNewlines)
  return String(masked.prefix(240))
}

var processSummaries: [[String: Any]] = []

func promptSnapshots() -> [PromptSnapshot] {
  processSummaries = []
  var snapshots: [PromptSnapshot] = []

  for target in processTargets() {
    let application = AXUIElementCreateApplication(target.pid)
    let windows = axChildren(application, kAXWindowsAttribute as CFString)
    var roots = windows.isEmpty ? [application] : windows
    var uniqueRoots: [AXUIElement] = []
    for root in roots where !uniqueRoots.contains(where: { sameElement($0, root) }) {
      uniqueRoots.append(root)
    }
    roots = uniqueRoots

    for (windowIndex, root) in roots.enumerated() {
      var seen: [AXUIElement] = []
      let rows = capture(root, seen: &seen)
      let snapshot = PromptSnapshot(target: target, root: root, rows: rows, text: promptText(rows))
      snapshots.append(snapshot)
      processSummaries.append([
        "pid": target.pid,
        "name": target.name,
        "bundle": target.bundle,
        "source": target.source,
        "windowIndex": windowIndex,
        "nodes": rows.count,
        "textPreview": maskedPreview(snapshot.text),
      ])
    }
  }

  return snapshots
}

func pressUniqueButton(
  in snapshot: PromptSnapshot,
  labels: [String],
  actionName: String
) throws -> String {
  let matches = exactButtons(in: snapshot.rows, labels: labels)
  guard matches.count == 1 else {
    throw CaptureFailure.message("verified Apple prompt has \(matches.count) exact \(actionName) buttons")
  }
  let result = AXUIElementPerformAction(matches[0].0, kAXPressAction as CFString)
  guard result == .success else {
    throw CaptureFailure.message("failed to press the unique \(actionName) button")
  }
  return matches[0].1
}

func writeOwnerOnlyCode(_ code: String, to path: String) throws {
  var bytes = Data(code.utf8)
  defer {
    if !bytes.isEmpty { bytes.resetBytes(in: 0..<bytes.count) }
  }
  guard bytes.count == 6 else { throw CaptureFailure.message("captured code is not six digits") }

  let descriptor = open(path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, S_IRUSR | S_IWUSR)
  guard descriptor >= 0 else {
    throw CaptureFailure.message("refused to create the owner-only challenge file")
  }

  var complete = false
  defer {
    close(descriptor)
    if !complete { unlink(path) }
  }

  var attributes = stat()
  guard fstat(descriptor, &attributes) == 0,
        (attributes.st_mode & S_IFMT) == S_IFREG,
        (attributes.st_mode & 0o077) == 0,
        attributes.st_uid == geteuid() else {
    throw CaptureFailure.message("challenge file ownership or permissions are unsafe")
  }

  let written = bytes.withUnsafeBytes { rawBuffer -> Int in
    guard let base = rawBuffer.baseAddress else { return -1 }
    return Darwin.write(descriptor, base, rawBuffer.count)
  }
  guard written == bytes.count, fsync(descriptor) == 0 else {
    throw CaptureFailure.message("failed to persist the challenge code")
  }
  complete = true
}

let trusted = AXIsProcessTrusted()
if preflightOnly {
  jsonPrint([
    "version": helperVersion,
    "ok": trusted,
    "accessibilityTrusted": trusted,
  ])
  exit(trusted ? 0 : 1)
}
var clicked: [String] = []
var clickedAllow = false
var clickedDone = false
var code = ""
var errorMessage: String?

if !trusted {
  errorMessage = "Accessibility permission is not granted"
} else {
  do {
    let deadline = Date().addingTimeInterval(waitSeconds)
    var verifiedPid: pid_t?

    while code.isEmpty && Date() < deadline {
      let snapshots = promptSnapshots()

      if clickAllow && !clickedAllow {
        let allowPrompts = snapshots.filter(isAppleTrustedDeviceAllowPrompt)
        if allowPrompts.count > 1 {
          throw CaptureFailure.message("multiple Apple trusted-device Allow prompts are visible")
        }
        if let allowPrompt = allowPrompts.first {
          let label = try pressUniqueButton(
            in: allowPrompt,
            labels: ["Allow"],
            actionName: "Allow"
          )
          clicked.append(label)
          clickedAllow = true
          verifiedPid = allowPrompt.target.pid
        }
      }

      let codePrompts = snapshots.filter(isAppleVerificationCodePrompt)
      if codePrompts.count > 1 {
        throw CaptureFailure.message("multiple Apple verification-code prompts are visible")
      }
      if let codePrompt = codePrompts.first {
        if let verifiedPid, codePrompt.target.pid != verifiedPid {
          throw CaptureFailure.message("Apple verification code appeared in a different process")
        }
        let candidates = sixDigitCodes(codePrompt.text)
        if candidates.count > 1 {
          throw CaptureFailure.message("Apple verification prompt contains multiple six-digit codes")
        }
        if let capturedCode = candidates.first {
          code = capturedCode
          if clickDone,
             let label = try? pressUniqueButton(
               in: codePrompt,
               labels: ["Done", "OK"],
               actionName: "Done/OK"
             ) {
            clicked.append(label)
            clickedDone = true
          }
          break
        }
      }

      Thread.sleep(forTimeInterval: 0.2)
    }

    guard !code.isEmpty else {
      throw CaptureFailure.message(
        "expected one Apple verification-code prompt with one code before the deadline"
      )
    }
    try writeOwnerOnlyCode(code, to: outputFile)
  } catch CaptureFailure.message(let message) {
    errorMessage = message
    code = ""
  } catch {
    errorMessage = "native Apple challenge capture failed closed"
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
  "codeCaptured": !code.isEmpty && errorMessage == nil,
  "outputFile": !code.isEmpty && errorMessage == nil ? outputFile : NSNull(),
  "error": errorMessage ?? NSNull(),
  "processes": processSummaries,
])
if errorMessage != nil {
  exit(1)
}
