import Foundation

/// Cross-platform reliable byte-stream boundary used by the session protocol.
///
/// A receive callback is an arbitrary stream chunk, not a protocol frame.  macOS currently supplies
/// Unix-domain and TLS/TCP implementations; the next iOS batch can supply its transport without
/// importing AppKit or the macOS service layer.
@MainActor
protocol SessionMessageLink: AnyObject {
    var onReceive: ((Data) -> Void)? { get set }
    var onClose: (() -> Void)? { get set }
    var isOpen: Bool { get }
    var isSynchronous: Bool { get }
    var pendingWriteBytes: Int { get }
    /// A classified terminal transport error, when the implementation has one.
    var terminalErrorDescription: String? { get }
    func send(_ framed: Data)
    /// Active close does not invoke this side's `onClose`.
    func close()
}

extension SessionMessageLink {
    var terminalErrorDescription: String? { nil }
}

/// Cross-platform construction seam shared by the macOS viewer and iOS crew data client.
@MainActor
protocol SessionMessageLinkConnecting {
    func connect() throws -> any SessionMessageLink
}

@MainActor
struct ClosureSessionMessageLinkConnector: SessionMessageLinkConnecting {
    private let operation: () throws -> any SessionMessageLink

    init(_ operation: @escaping () throws -> any SessionMessageLink) {
        self.operation = operation
    }

    func connect() throws -> any SessionMessageLink { try operation() }
}
