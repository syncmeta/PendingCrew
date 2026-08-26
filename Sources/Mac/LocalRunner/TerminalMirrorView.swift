#if os(macOS)
import AppKit
import Foundation
import SwiftTerm

/// **只负责画的那半**（spec `docs/internal/2026-08-19-backend-split-design.md` §5.2）。
///
/// 它是 `TerminalView` 的子类，**不是**那个会自己开进程的 SwiftTerm 基类 ——
/// 这半边不碰子进程。进程与权威
/// 缓冲区都在 `AgentSessionCore` 那边，这里只做三件事：
/// 1. 内核吐字节 → `feed(byteArray:)` 喂自己那份 `Terminal` → 画；
/// 2. 人敲键盘 / 粘贴 / 鼠标上报 → 回传给内核写进 PTY；
/// 3. 视口行列数变了 → 告诉内核，由它 resize 权威缓冲区并推 winsize。
///
/// 两侧各有一份 `Terminal`、喂同一批字节，所以选中复制 / 回滚 / reflow 全是
/// SwiftTerm 的原生代码路径，一行没改。一致性由
/// `Tests/PendingCrewTests/TerminalMirrorParityTests` 逐格钉住。
final class TerminalMirrorView: TerminalView, TerminalViewDelegate {

    /// 回滚历史行数上限。SwiftTerm 的默认值是 **500 行**（`TerminalOptions.scrollback`），
    /// agent session 跑一小会儿就顶满 —— 这是「往上滑只能滑一小段」的一半病根。
    /// 10000 行对齐 macOS Terminal.app 的默认历史长度。
    ///
    /// 代价（本机实测，见 Todo #34）：`CharData` stride 24B，每行占 `cols × 24B` ——
    /// 160 列约 3.8KB/行，装满约 38MB/session（100 列约 24MB）。而且**不是按需增长**：
    /// SwiftTerm 的 `Buffer.resize` 在列数变化时会遍历 `lines.maxLength` 把每个槽位都
    /// 实例化，所以窗口第一次改宽度就会一次性吃满这份内存。再往上取值不划算：50000 行
    /// 时单次改宽 resize 实测 28ms（10000 行是 4.5ms），拖窗口会掉帧。
    ///
    /// **2026-08-18 更正上面那段的一半**：「不是按需增长 / 窗口第一次改宽就一次性吃满」
    /// 是 **SwiftTerm 1.13** 的行为（`Buffer.resize` 遍历 `lines.maxLength`，下标 getter
    /// 给每个空槽位 `makeEmpty`）。本仓库现在锁的是 **1.18.0**，那段循环已改成只遍历
    /// `lines.count` —— 占用改回「按实际行数」，短命 session 不再预付整份容量，
    /// 那两个 resize 耗时数字同理也不再是「一次性全量」的量级。上面留 10000 行的
    /// **结论没变**（历史够长 + 窄列 reflow 有余量），只是理由的第二半过期了。
    ///
    /// 这个值只管**在跑**的 session。进程一终止就按 `TerminatedScrollbackPlan` 收下来
    /// （那里逐条回应了上面两条理由为什么对已终止的 session 不再成立）—— 停掉的 run
    /// 故意留在列表里给人回看，但没理由继续各占几十 MB。
    ///
    /// **两份缓冲区必须用同一个数**（否则 mirror 能滚到的历史比内核记得的多/少），
    /// 所以这里直接引用内核那个常量，不另写一个字面量。
    static let scrollbackLines = AgentSessionCore.scrollbackLines

    /// 内核 —— mirror 只画它、把人的输入回传给它，自己不碰进程。
    weak var core: AgentSessionCore?

    /// P2 remote 路径：mirror 仍在 app 侧，但键盘与尺寸不再直碰 core，改走协议。
    /// nil 时保留 P1 直连行为；这两个闭包也是传输层一行回退所需的兼容缝。
    var onSendBytes: (([UInt8]) -> Void)?
    var onResize: ((_ cols: Int, _ rows: Int) -> Void)?

    /// 视口行列数变化的旁路（门面接到内核的 `noteViewportChange()`）。
    var onViewportChange: (() -> Void)?

    /// 「用户主动滚」的判定要看最近有没有 PTY 输出；那个时刻归内核记。
    /// mirror 自己不再有 `lastOutputAt`。
    var remoteLastOutputAt: Date = .distantPast
    private var lastOutputAt: Date { core?.lastOutputAt ?? remoteLastOutputAt }

    /// 正在把内核的字节喂进自己那份 `Terminal`（见 `send(source:data:)`）。
    private var isReplayingCoreOutput = false

    /// 进程已终止 → 把回滚缓冲收到「够回看的尾巴」，其余槽位交还给系统。
    ///
    /// `changeScrollback` 会走到 `Buffer.changeHistorySize` → `CircularList.maxLength`
    /// 的 didSet：重建底层数组、只搬留下的那些行，多出来的 `BufferLine`（每行
    /// `cols × 24 B`）随旧数组一起释放。终端视图本身不动 —— 颜色/选择/复制/
    /// `inspect_session` 读画面全照旧。
    ///
    /// 幂等：再调一次算出同一个值，`changeHistorySize` 见 `newMaxLength == oldMaxLength`
    /// 直接 no-op。
    /// - Returns: 实际保留的行数（没收窄时返回当前上限）。
    @discardableResult
    func collapseScrollbackAfterExit() -> Int {
        let retained = TerminatedScrollbackPlan.retainedLines(
            rows: getTerminal().rows,
            thumbSize: Double(scrollThumbsize),
            canScroll: canScroll)
        let current = getTerminal().options.scrollback
        guard retained < current else { return current }
        changeScrollback(retained)
        return retained
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        // macOS 的 `TerminalView` 没有收 options 的构造器（`setupOptions` 自己 new 了一份
        // 默认 `TerminalOptions`），所以只能构造完再改。`changeScrollback` 会同时更新
        // `terminal.options.scrollback`，后续若走到 `Terminal.setup()` 重建 buffer 也保得住。
        getTerminal().changeScrollback(Self.scrollbackLines)
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        getTerminal().changeScrollback(Self.scrollbackLines)
    }

    /// 内核吐来的字节 —— **只有这条路径会喂 mirror**。
    ///
    /// 包一层是为了在喂的过程中拦掉 mirror 那份 `Terminal` 的「回主机」写：
    /// 同一批字节里的设备查询（DA / DSR / CPR / OSC 颜色查询…）两份缓冲区都会
    /// 各自生成一份回复，而 PTY 只该收到一份 —— 权威那份已经答了。
    func feedFromCore(_ slice: ArraySlice<UInt8>) {
        isReplayingCoreOutput = true
        defer { isReplayingCoreOutput = false }
        feed(byteArray: slice)
    }

    /// mirror 那份 `Terminal` 要往主机写字节。两种来源，处理不同：
    /// - **喂字节过程中**的回复 = 对设备查询的自动应答，权威那份已经答过 → 丢掉；
    /// - 其余（`Terminal.sendEvent` 的鼠标上报等，发生在 feed 之外）→ 照常上交，
    ///   由 `TerminalViewDelegate.send` 转给内核写进 PTY。
    override func send(source: Terminal, data: ArraySlice<UInt8>) {
        guard !isReplayingCoreOutput else { return }
        super.send(source: source, data: data)
    }

    /// 一个零宽/零高的 frame **不是真布局**，是 SwiftUI 重挂 NSView 时必经的那一拍
    /// （切 crew 就会发生，见下面 `sizeChanged` 处的注释）。但 SwiftTerm 照单
    /// 全收：`setFrameSize` → `processSizeChange` → `terminal.resize(cols: 0, …)`，被夹到
    /// `MINIMUM_COLS = 2`。2 列下 reflow 会把每条历史按 2 字宽重新折行（100 列的一行炸成
    /// ~50 行），行数瞬间冲破 scrollback 上限、顶部被**永久**裁掉；折回真实宽度时只能把
    /// 幸存的碎片拼回去，最老那条还是从半截字开始。这既吃掉历史也把排版拼乱 ——
    /// 实测（Todo #34）：100 列 400 行历史过一次 `.zero`，500 行上限下只剩 12 行、
    /// 10000 行上限下只剩 223 行，且首行都是断在半截的碎片。
    ///
    /// 所以零尺寸一律不往下传：视图保持上一次的真实尺寸，等真尺寸到了再走正常 resize。
    /// 用户自己把栏拖窄那种**真实**窄布局照常 reflow —— 那是终端应有的行为，不在这里拦。
    override func setFrameSize(_ newSize: NSSize) {
        guard Self.isRealLayout(newSize) else { return }
        super.setFrameSize(newSize)
    }

    /// 「这尺寸是不是一次真布局」的判定（`setFrameSize` 守卫的纯函数部分，供单测直接钉）。
    /// 只拦真正退化的尺寸（放不下任何一个字符格），不猜阈值。
    static func isRealLayout(_ size: NSSize) -> Bool {
        size.width >= 1 && size.height >= 1
    }

    // MARK: - 外置 overlay 滚动条支撑

    /// 终端滚动位置变化回调（外置 overlay 条据此刷新 knob）。
    /// `userInitiated` 区分「用户滚轮/拖动」与「输出自动滚屏」——只有前者才淡入显示条，
    /// 后者只静默同步几何，避免 agent 持续输出时条一直闪。
    var onScroll: ((_ userInitiated: Bool) -> Void)?

    /// SwiftTerm 每次 yDisp 变化都会调这里（用户滚 + 输出自动滚都算）。`scrollWheel` 在
    /// SwiftTerm 里非 open、外部模块不可 override，故不能直接标记「用户滚轮」；改用启发式：
    /// 输出驱动的自动滚屏总是紧跟一拍 `dataReceived`（lastOutputAt≈now），而用户滚轮/触控板
    /// 在空闲期滚动时最近没有 PTY 输出。>0.2s 没收到输出 = 判为用户主动 → 点亮条。
    override func scrolled(source terminal: Terminal, yDisp: Int) {
        super.scrolled(source: terminal, yDisp: yDisp)
        let userInitiated = Date().timeIntervalSince(lastOutputAt) > 0.2
        onScroll?(userInitiated)
    }

    /// SwiftTerm 内部那条焊死的 NSScroller —— 外置 overlay 条取代它，这里把它藏了。
    /// 网格宽度已扣掉 scrollerWidth（`getEffectiveWidth`），藏掉后右侧那条 ~15pt 空当
    /// 正好留给 overlay，且不占字。scroller 在 `setupScroller()` 里 addSubview，时机不定，
    /// 故 didAddSubview 抓一次 + 外部可再 sweep 一次兜底。
    private var didHideNativeScroller = false
    private var shouldHideNativeScroller = true
    override func didAddSubview(_ subview: NSView) {
        super.didAddSubview(subview)
        if let scroller = subview as? NSScroller {
            scroller.isHidden = shouldHideNativeScroller
            didHideNativeScroller = shouldHideNativeScroller
        }
    }

    /// Agent TUI 使用外置 overlay；普通终端保留 SwiftTerm 原生滚动条。
    func useNativeScroller() {
        shouldHideNativeScroller = false
        didHideNativeScroller = false
        for case let scroller as NSScroller in subviews { scroller.isHidden = false }
    }

    /// 兜底扫一遍 subviews 藏掉 NSScroller；返回是否已确认藏到（供 fail-loud 用）。
    @discardableResult
    func hideNativeScroller() -> Bool {
        shouldHideNativeScroller = true
        if didHideNativeScroller { return true }
        if let native = subviews.compactMap({ $0 as? NSScroller }).first {
            native.isHidden = true
            didHideNativeScroller = true
        }
        return didHideNativeScroller
    }

    // MARK: - TerminalViewDelegate

    /// 人敲键盘 / 粘贴 / 鼠标上报 → 回传给内核写进 PTY。
    func send(source: TerminalView, data: ArraySlice<UInt8>) {
        let bytes = Array(data)
        if let onSendBytes { onSendBytes(bytes) } else { core?.write(bytes) }
    }

    /// 视口行列数变了 → 告诉内核，由它 resize 自己那份缓冲区并推 winsize。
    /// **mirror 不直接碰 PTY** —— PTY 的属主是内核（P4 之后它在另一个进程里）。
    ///
    /// 顺带打一次「视口变了」的宽限窗：切 crew 让终端视图卸载/重挂 → SwiftUI
    /// 必经 frame .zero → 真实尺寸 → 两次行列数变化 → 两次 SIGWINCH。子进程随后
    /// 吐的整屏重绘**不是它在干活**，是我们要求它重画的（Todo #32）。
    func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
        if let onResize {
            onResize(newCols, newRows)
        } else {
            core?.resize(cols: newCols, rows: newRows)
            core?.noteViewportChange()
        }
        onViewportChange?()
    }

    func setTerminalTitle(source: TerminalView, title: String) {}
    func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
    /// 外置 overlay 的几何刷新走 `onScroll`（上面那条 `scrolled(source:yDisp:)`），
    /// 这条协议回调不再另做事。
    func scrolled(source: TerminalView, position: Double) {}
    func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}

    /// SwiftTerm 那个会自己开进程的基类自带这两条剪贴板桥接（OSC 52）；
    /// 它不再是我们的基类，所以逐字搬过来，别让 OSC 52 静默失效。
    func clipboardCopy(source: TerminalView, content: Data) {
        if let str = String(bytes: content, encoding: .utf8) {
            let pasteBoard = NSPasteboard.general
            pasteBoard.clearContents()
            pasteBoard.writeObjects([str as NSString])
        }
    }

    func clipboardRead(source: TerminalView) -> Data? {
        guard let str = NSPasteboard.general.string(forType: .string) else { return nil }
        return str.data(using: .utf8)
    }
}
#endif
