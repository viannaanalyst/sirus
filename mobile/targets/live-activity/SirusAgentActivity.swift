import ActivityKit
import SwiftUI
import WidgetKit

/// The same shape as the app's own copy (`SirusNativeControlsModule.swift`): ActivityKit matches
/// them by name and coding.
struct SirusAgentAttributes: ActivityAttributes {
  struct ContentState: Codable, Hashable {
    var startedAt: Date
    var finished: Bool
  }
  var sessionId: String
  var title: String
  var project: String
}

private let accent = Color(red: 0.85, green: 0.31, blue: 0.60)

private struct Status: View {
  let state: SirusAgentAttributes.ContentState
  let stale: Bool
  var body: some View {
    if state.finished {
      Text("Terminou").foregroundStyle(.secondary)
    } else if stale {
      Text("Abra o Sirus para atualizar").foregroundStyle(.secondary)
    } else {
      Text(timerInterval: state.startedAt...Date.distantFuture, countsDown: false)
        .monospacedDigit()
        .foregroundStyle(.secondary)
    }
  }
}

private struct Mark: View {
  let finished: Bool
  var body: some View {
    Image(systemName: finished ? "checkmark.circle.fill" : "sparkles")
      .foregroundStyle(accent)
  }
}

struct SirusAgentActivityWidget: Widget {
  var body: some WidgetConfiguration {
    ActivityConfiguration(for: SirusAgentAttributes.self) { context in
      HStack(spacing: 12) {
        Mark(finished: context.state.finished).font(.title3)
        VStack(alignment: .leading, spacing: 2) {
          Text(context.attributes.title).font(.subheadline.weight(.semibold)).lineLimit(1)
          Text(context.attributes.project).font(.caption).foregroundStyle(.secondary).lineLimit(1)
        }
        Spacer(minLength: 8)
        Status(state: context.state, stale: context.isStale).font(.subheadline)
      }
      .padding(16)
      .activityBackgroundTint(Color.black.opacity(0.6))
      .activitySystemActionForegroundColor(.white)
    } dynamicIsland: { context in
      DynamicIsland {
        DynamicIslandExpandedRegion(.leading) { Mark(finished: context.state.finished).font(.title3) }
        DynamicIslandExpandedRegion(.center) {
          Text(context.attributes.title).font(.subheadline.weight(.semibold)).lineLimit(1)
        }
        DynamicIslandExpandedRegion(.trailing) {
          Status(state: context.state, stale: context.isStale).font(.caption).frame(maxWidth: 64)
        }
        DynamicIslandExpandedRegion(.bottom) {
          Text(context.attributes.project).font(.caption).foregroundStyle(.secondary).lineLimit(1)
        }
      } compactLeading: {
        Mark(finished: context.state.finished)
      } compactTrailing: {
        Status(state: context.state, stale: context.isStale).font(.caption2).frame(maxWidth: 44)
      } minimal: {
        Mark(finished: context.state.finished)
      }
    }
  }
}

@main
struct SirusAgentActivityBundle: WidgetBundle {
  var body: some Widget { SirusAgentActivityWidget() }
}
