import Foundation

public enum Scenario: String, CaseIterable {
    case success = "Successful dictation"
    case microphone = "Microphone permission"
    case model = "Missing model"
    case silence = "No speech"
    case insertion = "Insertion failure"
}

public enum Stage: Equatable {
    case idle, listening, processing, completed, canceled
    case failed(Scenario)
}

public enum WorkMode: String, CaseIterable {
    case clean = "Clean", memo = "Memo", code = "Code", email = "Email"

    public var cliName: String {
        switch self {
        case .clean: return "clean"
        case .memo: return "memo"
        case .code: return "coding-prompt"
        case .email: return "email-reply"
        }
    }
}

/// Synthetic interaction model only. Production text processing remains in Rust.
public struct Session {
    public private(set) var stage: Stage = .idle
    public private(set) var token = UUID()
    public private(set) var scenario: Scenario = .success
    public private(set) var mode: WorkMode = .clean
    public let original = "Um, let's move the review to Thursday."
    public let cleaned = "Let's move the review to Thursday."

    public init() {}

    public var hasTranscript: Bool {
        stage == .completed || stage == .failed(.insertion)
    }

    @discardableResult
    public mutating func start(scenario: Scenario, mode: WorkMode) -> UUID {
        token = UUID()
        self.scenario = scenario
        self.mode = mode
        switch scenario {
        case .microphone, .model: stage = .failed(scenario)
        default: stage = .listening
        }
        return token
    }

    public mutating func stop() {
        guard stage == .listening else { return }
        stage = .processing
    }

    public mutating func finish(token: UUID) {
        guard self.token == token, stage == .processing else { return }
        stage = scenario == .silence ? .failed(.silence) : .completed
    }

    public mutating func cancel() {
        guard stage == .listening || stage == .processing else { return }
        token = UUID()
        stage = .canceled
    }

    public mutating func simulateInsert() {
        guard hasTranscript else { return }
        // Preview never sends keyboard events or writes into another application.
        stage = .failed(.insertion)
    }

    public mutating func reset() {
        token = UUID()
        stage = .idle
    }
}

public enum PaletteAction: String, CaseIterable {
    case dictate = "Start dictation"
    case file = "Transcribe audio…"
    case history = "Search history"
    case vocabulary = "Vocabulary & snippets"
    case review = "Review last transcript"

    public static func matching(_ query: String) -> [Self] {
        allCases.filter { query.isEmpty || $0.rawValue.localizedCaseInsensitiveContains(query) }
    }

    public static func move(_ index: Int, by delta: Int, count: Int) -> Int {
        guard count > 0 else { return 0 }
        return ((index + delta) % count + count) % count
    }
}
