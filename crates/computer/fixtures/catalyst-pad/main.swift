// The computer-use suite's Mac Catalyst fixture (docs/COMPUTER-USE-PLAN.md §8, Phase 5, stream B).
//
// UIKit, built for the Mac with `swiftc -target <arch>-apple-ios<ver>-macabi`: a stepper, a text
// field and two buttons. Every control writes what happened to it to a log, one JSON object a
// line, as the other fixtures do.
//
//   catalyst-pad <log path>
//
// It never activates itself: it is opened in the background (`open -n -g`) and its window is
// ordered behind every other window through AppKit, which UIKit on the Mac runs on. SIGUSR1 logs
// every control's state; SIGTERM quits.

import UIKit

let logPath = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "/tmp/catalyst-pad.log"
FileManager.default.createFile(atPath: logPath, contents: nil)
let logHandle = FileHandle(forWritingAtPath: logPath)!

func log(_ id: String, _ ev: String, v: String? = nil) {
  var o: [String: Any] = ["t": Date().timeIntervalSince1970 * 1000, "id": id, "ev": ev]
  if let v { o["v"] = v }
  let data = try! JSONSerialization.data(withJSONObject: o, options: [.sortedKeys])
  logHandle.write(data + Data([0x0a]))
}

/// Orders every window of the app behind the others through AppKit's NSApplication, which UIKit
/// apps on the Mac run on but can't name.
func orderBack() {
  guard let cls = NSClassFromString("NSApplication") as? NSObject.Type,
    let app = cls.perform(NSSelectorFromString("sharedApplication"))?.takeUnretainedValue() as? NSObject,
    let windows = app.value(forKey: "windows") as? [NSObject]
  else { log("window", "not-ordered"); return }
  for w in windows { w.perform(NSSelectorFromString("orderBack:"), with: nil) }
  log("window", "ordered-back", v: String(windows.count))
}

final class PadController: UIViewController, UITextFieldDelegate {
  let stepper = UIStepper()
  let countLabel = UILabel()
  let field = UITextField()
  let status = UILabel()
  var submitted = ""
  var taps = 0

  override func viewDidLoad() {
    super.viewDidLoad()
    view.backgroundColor = .systemBackground
    let title = UILabel()
    title.text = "Order"
    title.font = .boldSystemFont(ofSize: 18)
    stepper.minimumValue = 0
    stepper.maximumValue = 20
    stepper.value = 1
    stepper.accessibilityLabel = "Quantity"
    stepper.addTarget(self, action: #selector(stepped), for: .valueChanged)
    countLabel.text = "Quantity: 1"
    field.placeholder = "Note"
    field.accessibilityLabel = "Note"
    field.borderStyle = .roundedRect
    field.addTarget(self, action: #selector(edited), for: .editingChanged)
    let submit = UIButton(type: .system)
    submit.setTitle("Place Order", for: .normal)
    submit.addTarget(self, action: #selector(placed), for: .primaryActionTriggered)
    let tap = UIButton(type: .system)
    tap.setTitle("Ping", for: .normal)
    tap.addTarget(self, action: #selector(pinged), for: .primaryActionTriggered)
    status.text = "No order yet"
    let row = UIStackView(arrangedSubviews: [countLabel, stepper])
    row.spacing = 12
    let buttons = UIStackView(arrangedSubviews: [submit, tap])
    buttons.spacing = 16
    let stack = UIStackView(arrangedSubviews: [title, row, field, buttons, status])
    stack.axis = .vertical
    stack.alignment = .leading
    stack.spacing = 14
    stack.translatesAutoresizingMaskIntoConstraints = false
    view.addSubview(stack)
    NSLayoutConstraint.activate([
      stack.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 20),
      stack.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 20),
      field.widthAnchor.constraint(equalToConstant: 240),
    ])
  }

  @objc func stepped() {
    countLabel.text = "Quantity: \(Int(stepper.value))"
    log("quantity", "value", v: String(Int(stepper.value)))
  }
  @objc func edited() { log("note", "text", v: field.text ?? "") }
  @objc func placed() {
    submitted = "\(Int(stepper.value))|\(field.text ?? "")"
    status.text = "Order placed"
    log("place-order", "press", v: submitted)
  }
  @objc func pinged() {
    taps += 1
    log("ping", "press", v: String(taps))
  }
  var state: [String: String] {
    ["quantity": String(Int(stepper.value)), "note": field.text ?? "", "submitted": submitted, "pings": String(taps)]
  }
}

var controller: PadController?

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
  var window: UIWindow?
  func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options: UIScene.ConnectionOptions) {
    guard let ws = scene as? UIWindowScene else { return }
    ws.title = "Catalyst Pad"
    ws.sizeRestrictions?.minimumSize = CGSize(width: 420, height: 300)
    ws.sizeRestrictions?.maximumSize = CGSize(width: 420, height: 300)
    let w = UIWindow(windowScene: ws)
    let c = PadController()
    controller = c
    w.rootViewController = c
    w.makeKeyAndVisible()
    window = w
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { orderBack() }
  }
}

final class AppDelegate: UIResponder, UIApplicationDelegate {
  func application(_ a: UIApplication, didFinishLaunchingWithOptions o: [UIApplication.LaunchOptionsKey: Any]?) -> Bool {
    if let p = ProcessInfo.processInfo.environment["FIXTURE_PID_FILE"] {
      try? String(ProcessInfo.processInfo.processIdentifier).write(toFile: p, atomically: true, encoding: .utf8)
    }
    log("app", "ready", v: String(ProcessInfo.processInfo.processIdentifier))
    return true
  }
  func application(_ a: UIApplication, configurationForConnecting s: UISceneSession, options: UIScene.ConnectionOptions) -> UISceneConfiguration {
    let c = UISceneConfiguration(name: nil, sessionRole: s.role)
    c.delegateClass = SceneDelegate.self
    return c
  }
}

signal(SIGUSR1, SIG_IGN)
let snapshot = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
snapshot.setEventHandler {
  let state = controller?.state ?? [:]
  let data = try! JSONSerialization.data(withJSONObject: state, options: [.sortedKeys])
  log("state", "snapshot", v: String(data: data, encoding: .utf8)!)
}
snapshot.resume()
signal(SIGTERM, SIG_IGN)
let quit = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
quit.setEventHandler { log("app", "quit"); exit(0) }
quit.resume()

UIApplicationMain(CommandLine.argc, CommandLine.unsafeArgv, nil, NSStringFromClass(AppDelegate.self))
