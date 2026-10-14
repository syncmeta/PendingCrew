#if os(iOS)
import SwiftUI
import UIKit
import SwiftTerm

/// Renderer inside the original mobile session navigation. The authenticated
/// crewd socket is the only stream/input authority; renderer replies are local.
struct CrewdMobileTerminalView: UIViewRepresentable {
    @ObservedObject var control: CrewdPublicTerminal
    func makeCoordinator() -> Coordinator { Coordinator(control: control) }
    func makeUIView(context: Context) -> TerminalView {
        let view = TerminalView(frame: .zero)
        view.terminalDelegate = context.coordinator
        context.coordinator.install(view)
        return view
    }
    func updateUIView(_ view: TerminalView, context: Context) {
        context.coordinator.control = control
        context.coordinator.renderSnapshot(view)
    }
    static func dismantleUIView(_ view: TerminalView, coordinator: Coordinator) {
        coordinator.control.onSnapshot = nil; coordinator.control.onBytes = nil
        view.terminalDelegate = nil
    }
    @MainActor final class Coordinator: NSObject, TerminalViewDelegate {
        var control: CrewdPublicTerminal
        private var feeding = false
        private var renderedCut: String?
        init(control: CrewdPublicTerminal) { self.control = control }
        func install(_ view: TerminalView) {
            control.onSnapshot = { [weak self, weak view] _ in
                guard let self, let view else { return }; self.renderSnapshot(view)
            }
            control.onBytes = { [weak self, weak view] bytes in
                guard let self, let view else { return }; self.feed(view, bytes)
            }
            renderSnapshot(view)
        }
        func renderSnapshot(_ view: TerminalView) {
            guard let snapshot = control.snapshot else { renderedCut = nil; return }
            let cut = snapshot.target.generation + ":" + snapshot.nextCursor
            guard renderedCut != cut else { return }; renderedCut = cut
            feed(view, Data([27, 91, 50, 74, 27, 91, 72]) + (snapshot.bytes ?? Data((snapshot.text ?? "").utf8)))
        }
        private func feed(_ view: TerminalView, _ bytes: Data) {
            feeding = true; defer { feeding = false }
            let bytes = Array(bytes); view.feed(byteArray: bytes[...])
        }
        func send(source: TerminalView, data: ArraySlice<UInt8>) {
            guard !feeding else { return }; control.enqueueInput(Data(data))
        }
        func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
            guard !feeding else { return }
            Task { await control.resize(columns: newCols, rows: newRows) }
        }
        func setTerminalTitle(source: TerminalView, title: String) {}
        func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
        func scrolled(source: TerminalView, position: Double) {}
        func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {}
        func bell(source: TerminalView) {}
    }
}
#endif
