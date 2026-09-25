import PrototypeCore

// Standalone checks also run on Macs with Command Line Tools but no Xcode/XCTest.
func expectEqual<T: Equatable>(_ actual: T, _ expected: T, _ message: String = "", file: StaticString = #file, line: UInt = #line) {
    precondition(actual == expected, "Expected \(expected), got \(actual). \(message)", file: file, line: line)
}
func expectTrue(_ value: Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) {
    precondition(value, message, file: file, line: line)
}
func expectFalse(_ value: Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) {
    precondition(!value, message, file: file, line: line)
}

@main
struct SessionTests {
    static func main() {
        runHotkeyTests()
        let suite = SessionTests()
        suite.testPrimaryFlowPreservesOriginalAndMode()
        suite.testCancelDiscardsPendingCompletionAndRestartRejectsOldResult()
        suite.testPermissionAndModelPreventCapture()
        suite.testNoSpeechAndInsertionFailureHaveDifferentRetention()
        suite.testResetInvalidatesPendingWorkAndInvalidTransitionsDoNothing()
        suite.testPaletteFilteringAndKeyboardWrapIncludingEmptyResults()
        suite.testCopiedStatusFollowsTheCopiedVariantOnly()
        print("PASS: 7 prototype state and keyboard checks")
    }
    func testPrimaryFlowPreservesOriginalAndMode() {
        var session = Session()
        expectEqual(session.stage, .idle)
        let token = session.start(scenario: .success, mode: .code)
        expectEqual(session.stage, .listening)
        expectFalse(session.hasTranscript)
        session.stop()
        expectEqual(session.stage, .processing)
        session.finish(token: token)
        expectTrue(session.hasTranscript)
        expectEqual(session.mode.cliName, "coding-prompt")
        expectTrue(session.original.hasPrefix("Um,"))
        expectFalse(session.cleaned.hasPrefix("Um,"))
    }

    func testCancelDiscardsPendingCompletionAndRestartRejectsOldResult() {
        var session = Session()
        let old = session.start(scenario: .success, mode: .clean)
        session.stop()
        session.cancel()
        session.finish(token: old)
        expectEqual(session.stage, .canceled)
        expectFalse(session.hasTranscript)
        let current = session.start(scenario: .success, mode: .memo)
        session.stop()
        session.finish(token: old)
        expectEqual(session.stage, .processing)
        session.finish(token: current)
        expectEqual(session.stage, .completed)
    }

    func testPermissionAndModelPreventCapture() {
        for scenario in [Scenario.microphone, .model] {
            var session = Session()
            let token = session.start(scenario: scenario, mode: .clean)
            expectEqual(session.stage, .failed(scenario))
            session.stop()
            session.finish(token: token)
            expectEqual(session.stage, .failed(scenario))
            expectFalse(session.hasTranscript)
        }
    }

    func testNoSpeechAndInsertionFailureHaveDifferentRetention() {
        var session = Session()
        let token = session.start(scenario: .silence, mode: .clean)
        session.stop()
        session.finish(token: token)
        expectEqual(session.stage, .failed(.silence))
        expectFalse(session.hasTranscript)
        let second = session.start(scenario: .insertion, mode: .clean)
        session.stop()
        session.finish(token: second)
        session.simulateInsert()
        expectEqual(session.stage, .failed(.insertion))
        expectTrue(session.hasTranscript, "Copy fallback must preserve output")
    }

    func testResetInvalidatesPendingWorkAndInvalidTransitionsDoNothing() {
        var session = Session()
        session.stop()
        session.cancel()
        session.simulateInsert()
        expectEqual(session.stage, .idle)
        let token = session.start(scenario: .success, mode: .email)
        session.finish(token: token)
        expectEqual(session.stage, .listening)
        session.stop()
        session.reset()
        session.finish(token: token)
        expectEqual(session.stage, .idle)
        expectFalse(session.hasTranscript)
    }

    func testPaletteFilteringAndKeyboardWrapIncludingEmptyResults() {
        expectEqual(PaletteAction.matching("HISTORY"), [.history])
        expectEqual(PaletteAction.matching("zzzz"), [])
        expectEqual(PaletteAction.move(0, by: -1, count: 5), 4)
        expectEqual(PaletteAction.move(4, by: 1, count: 5), 0)
        expectEqual(PaletteAction.move(0, by: 1, count: 0), 0)
    }

    func testCopiedStatusFollowsTheCopiedVariantOnly() {
        // Regression: copying Cleaned then switching to Original must not claim
        // Original is on the clipboard.
        var status = CopyStatus()
        expectFalse(status.isCopied(original: false))
        status.record(original: false, succeeded: true)
        expectTrue(status.isCopied(original: false))
        expectFalse(status.isCopied(original: true))
        status.record(original: true, succeeded: true)
        expectTrue(status.isCopied(original: true))
        expectFalse(status.isCopied(original: false))
        status.record(original: true, succeeded: false)
        expectTrue(status.failed)
        expectFalse(status.isCopied(original: true))
        expectFalse(status.isCopied(original: false))
        status.record(original: false, succeeded: true)
        expectFalse(status.failed)
    }
}
