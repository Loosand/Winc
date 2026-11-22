import AppKit

let expected = URL(fileURLWithPath: CommandLine.arguments[2]).resolvingSymlinksInPath()
let running = NSRunningApplication.runningApplications(withBundleIdentifier: CommandLine.arguments[1])
    .filter { $0.bundleURL?.resolvingSymlinksInPath().path == expected.path }
for app in running where !app.isTerminated {
    guard app.terminate() else { exit(1) }
}
let deadline = Date(timeIntervalSinceNow: 12)
while running.contains(where: { !$0.isTerminated }) {
    guard Date() < deadline else { exit(1) }
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
}
