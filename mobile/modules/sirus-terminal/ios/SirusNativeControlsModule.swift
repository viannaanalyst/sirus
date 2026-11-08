import ActivityKit
import ExpoModulesCore
import UIKit

/// The Live Activity's data, as the extension (`targets/live-activity`) reads it.
struct SirusAgentAttributes: ActivityAttributes {
  struct ContentState: Codable, Hashable {
    var startedAt: Date
    var finished: Bool
  }
  var sessionId: String
  var title: String
  var project: String
}

/// Without a server push the app can't reach an activity once iOS suspends it, so it goes stale
/// and says to open the app rather than go on claiming that the agent works.
private let staleAfter: TimeInterval = 10 * 60
/// A finished activity stays on the lock screen this long.
private let finishedFor: TimeInterval = 15 * 60

@available(iOS 16.2, *)
private func showAgent(sessionId: String, title: String, project: String, startedAt: Double) async {
  let state = SirusAgentAttributes.ContentState(startedAt: Date(timeIntervalSince1970: startedAt / 1000), finished: false)
  let content = ActivityContent(state: state, staleDate: Date().addingTimeInterval(staleAfter))
  if let running = Activity<SirusAgentAttributes>.activities.first(where: { $0.attributes.sessionId == sessionId }) {
    await running.update(content)
    return
  }
  guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }
  let attributes = SirusAgentAttributes(sessionId: sessionId, title: title, project: project)
  _ = try? Activity.request(attributes: attributes, content: content)
}

@available(iOS 16.2, *)
private func finishAgents(except kept: [String]) async {
  for activity in Activity<SirusAgentAttributes>.activities where !kept.contains(activity.attributes.sessionId) {
    var state = activity.content.state
    state.finished = true
    await activity.end(ActivityContent(state: state, staleDate: nil), dismissalPolicy: .after(Date().addingTimeInterval(finishedFor)))
  }
}

/// Small native controls the app draws from its own theme (`SirusNativeControls` in JS).
public class SirusNativeControlsModule: Module {
  public func definition() -> ModuleDefinition {
    Name("SirusNativeControls")

    AsyncFunction("showAgentActivity") { (sessionId: String, title: String, project: String, startedAt: Double) in
      if #available(iOS 16.2, *) {
        await showAgent(sessionId: sessionId, title: title, project: project, startedAt: startedAt)
      }
    }
    AsyncFunction("finishAgentActivities") { (kept: [String]) in
      if #available(iOS 16.2, *) {
        await finishAgents(except: kept)
      }
    }

    View(SirusSegmentedControlView.self) {
      ViewName("SegmentedControl")
      Events("onSelectionChange")
      Prop("labels") { (view: SirusSegmentedControlView, value: [String]) in view.labels = value }
      Prop("selectedIndex") { (view: SirusSegmentedControlView, value: Int) in view.selectedIndex = value }
      Prop("trackColor") { (view: SirusSegmentedControlView, value: UIColor) in view.trackColor = value }
      Prop("selectedColor") { (view: SirusSegmentedControlView, value: UIColor) in view.selectedColor = value }
      Prop("textColor") { (view: SirusSegmentedControlView, value: UIColor) in view.textColor = value }
      Prop("mutedColor") { (view: SirusSegmentedControlView, value: UIColor) in view.mutedColor = value }
      Prop("appearance") { (view: SirusSegmentedControlView, value: String) in view.appearance = value }
    }
  }
}
