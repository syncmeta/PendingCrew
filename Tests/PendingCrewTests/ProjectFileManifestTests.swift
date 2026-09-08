import XCTest
import Foundation

/// **提交进来的 pbxproj 必须和磁盘上的源码文件对得上** —— 一条会红的尺子，
/// 代替 `scripts/gen-project.sh` 开头那句「改了 project.yml 就跑这个，别直接跑 xcodegen」。
///
/// ## 病根：那句注释只在被读到时才起作用，而这个仓库每天起新 worker
///
/// 2026-09-08 有一个 crew 的机长**连续四次**直接跑裸 `xcodegen generate`，绕过了
/// `gen-project.sh`。它是自己回头查才发现的 —— 没人拦它，产物上也看不出来。
/// 失效的原因不是谁不守规矩，是**新来的人不知道那个脚本存在**。
///
/// ## 这条尺子量的是什么（以及为什么不是「版本」）
///
/// 先说一个实测结论，它决定了这条尺子的形状：**本机那份 2.45.4 和仓库钉住的
/// 2.46.0，对当前这份 `project.yml` 的输出逐字节相同**（2026-09-08 实测，diff 0 行）。
/// 所以「谁跑的生成器」这件事**在产物上根本不留痕迹，原理上就测不出来** ——
/// 而且它也不值得测：字节一样就等于没出事。
///
/// 真正会出事、而且**本机永远看不到红**的是另一件事：**加了 Swift 文件却没 regen。**
/// 加文件的人自己能编（Xcode 会自己发现磁盘上的文件），别人 clone 下来那份
/// pbxproj 里没有它，**只在别人机器上炸**。这就是这条尺子钉的东西。
///
/// 所以判据是「磁盘上的每个 .swift 都在 pbxproj 里」。
///
/// ## 只查一个方向，因为反方向**结构上不可能红**（实测，不是推断）
///
/// 反方向（pbxproj 引用了磁盘上没有的文件）我写过、也造过红样本，结论是
/// **它永远轮不到发声**：2026-09-08 实测，往 pbxproj 里塞一个磁盘上不存在的
/// `.swift`，`xcodebuild` 在编译阶段就直接停：
///
///     error: Build input file cannot be found: '.../ZZDanglingDecoy.swift'
///
/// 测试根本没跑到。也就是说那件事**编译器已经抓了，而且比测试更早、更响**。
/// 留一条永远不会红的断言比不留更糟 —— 它看起来像覆盖，其实是空的。
/// 所以这里把它删掉，并把这次实验写在这儿，免得下一个人「补全对称性」时再加回来。
///
/// ## 为什么不是「落一份指纹文件」
///
/// 指纹（版本 + pbxproj 的 sha）会**比这条尺子更弱**：
///   1. 它对「加了文件没 regen」完全瞎 —— 那种情况下 pbxproj 一个字节没动，
///      指纹当然也对得上，全绿。而那恰好是唯一一种本机看不到红的错。
///   2. 它是**可以手改的第二份真值** —— 裸跑之后顺手更新指纹就绿了。
///   3. 它记的版本号，按上面那条实测，在这个仓库里区分不出任何东西。
/// 仓库里已经有一条形状正确的：CI 的 `project-drift` job（`.github/workflows/ci.yml`）
/// 按 `.xcodegen-version` 重新生成再 `git diff --exit-code`。那条更严、且不可伪造。
/// 这里不发明第二种判据，只补它够不到的地方 —— 它要 push 上 GitHub 才跑，
/// 而这个仓库的常态是本地 main 领先 origin 很多笔。
///
/// ## 边界（这条尺子抓不到的）
///
///   · **同名文件在目录之间搬家**：集合按 basename 比，A/Foo.swift → B/Foo.swift
///     两边集合都不变 ⇒ 绿。（pbxproj 的完整路径散在 PBXGroup 的嵌套里，
///     重建它要把整棵组树解出来；那点收益不值这份复杂度和它带来的噪音。）
///   · **用别的版本生成器跑出不同字节**：本条不查内容、只查文件名集合。
///     那件事归 CI 的 `project-drift`。
///   · **Fixtures/**：`project.yml` 明确 excludes 它（那份数据不入 git），
///     所以这里也跳过，否则取过 fixture 的机器会假红。
final class ProjectFileManifestTests: XCTestCase {

    // MARK: - 两个方向

    /// 磁盘上有、pbxproj 里没有 ⇒ **加了 Swift 文件没跑 `scripts/gen-project.sh`**。
    /// 这是 #490 那次的病，也是唯一一种「提交者自己能编、别人 clone 下来编不过」的错。
    func test_磁盘上的每个swift文件都在pbxproj里() throws {
        let root = try Self.repoRoot()
        let onDisk = try Self.swiftBasenamesOnDisk(root: root)
        let inProject = try Self.swiftBasenamesInPbxproj(root: root)

        // 防止尺子空转报绿：解析器一旦坏掉，两个集合都会变空，然后「差集为空」==全绿。
        XCTAssertGreaterThan(onDisk.count, 300,
                             "磁盘扫描只找到 \(onDisk.count) 个 .swift —— 尺子本身失效了，不是仓库变干净了")
        XCTAssertGreaterThan(inProject.count, 300,
                             "pbxproj 解析只找到 \(inProject.count) 个 .swift —— 尺子本身失效了")

        let missing = onDisk.subtracting(inProject).sorted()
        XCTAssertEqual(missing, [], """
            这些 .swift 文件在磁盘上，但**没进 pbxproj** —— 你本机能编（Xcode 自己发现了它们），
            别人 clone 下来那份工程里没有它们，**只在别人机器上炸**：
            \(missing.joined(separator: "\n"))

            修法：跑 `scripts/gen-project.sh`（**不是裸 `xcodegen generate`** —— 那会用你本机
            brew 装的那一版，而版本的唯一真值是仓库根的 .xcodegen-version），
            然后把 PendingCrew.xcodeproj/project.pbxproj 一起提交。
            本机版本对不上时用 `scripts/gen-project.sh --fetch`。
            """)
    }

    // MARK: - 尺子自己的自检
    //
    // 这几条不碰仓库现状，只喂合成输入 —— 它们保证上面两条**红得起来**：
    // 解析器要是把什么都解析成空集，上面两条的差集永远为空、永远绿。

    func test_自检_从projectyml取出源码根() {
        let sample = """
        targets:
          App:
            sources:
              - path: Sources
              - path: Shared/AppUpdate
            settings:
              base:
                SWIFT_VERSION: "5.0"
          Tests:
            sources:
              - path: Tests/AppTests
                excludes:
                  - "Fixtures"
                  - "Fixtures/**"
              - path: Sources/One.swift
        """
        XCTAssertEqual(Self.sourcePaths(inProjectYAML: sample),
                       ["Sources", "Shared/AppUpdate", "Tests/AppTests", "Sources/One.swift"],
                       "源码根解析错了 —— 少一个根就等于那一整片目录不再被看住")
    }

    /// `excludes:` 底下那两行也是 `- "..."` 开头，但它们**不是源码根**。
    /// 第一版就把它们当成根收进来过，于是尺子会去磁盘上找一个叫 `Fixtures` 的根。
    func test_自检_excludes里的条目不算源码根() {
        let sample = """
            sources:
              - path: Tests/AppTests
                excludes:
                  - "Fixtures"
                  - "Fixtures/**"
        """
        XCTAssertEqual(Self.sourcePaths(inProjectYAML: sample), ["Tests/AppTests"])
    }

    /// `sources:` 块结束之后的 `- path:`（比如 `dependencies:` 底下的）不算数。
    func test_自检_sources块之外的path不算() {
        let sample = """
          App:
            sources:
              - path: Sources
            dependencies:
              - path: NotASourceRoot
        """
        XCTAssertEqual(Self.sourcePaths(inProjectYAML: sample), ["Sources"])
    }

    func test_自检_从pbxproj取出swift文件名() {
        let sample = """
        		AA11 /* Foo.swift */ = {isa = PBXFileReference; path = Foo.swift; sourceTree = "<group>"; };
        		BB22 /* Bar Baz.swift */ = {isa = PBXFileReference; path = "Bar Baz.swift"; sourceTree = "<group>"; };
        		CC33 /* Assets */ = {isa = PBXFileReference; path = Assets.xcassets; sourceTree = "<group>"; };
        		DD44 /* Dir */ = {isa = PBXGroup; path = SomeDir; sourceTree = "<group>"; };
        """
        XCTAssertEqual(Self.swiftBasenames(inPbxproj: sample), ["Foo.swift", "Bar Baz.swift"],
                       "带空格的文件名在 pbxproj 里是带引号的，引号必须剥掉；非 .swift 不收")
    }

    /// **红样本**：尺子对「磁盘多一个 pbxproj 没有的文件」这件事真的会红。
    /// 用的是集合运算本身，不依赖仓库当下的状态 —— 仓库是绿的，所以只能合成一个。
    func test_自检_尺子对缺失文件会红() {
        let onDisk: Set<String> = ["A.swift", "B.swift", "新加的.swift"]
        let inProject: Set<String> = ["A.swift", "B.swift"]
        XCTAssertEqual(onDisk.subtracting(inProject).sorted(), ["新加的.swift"],
                       "这一条要是绿不了，上面那条就是永远不会红的摆设")
    }

    // MARK: - 解析

    /// 从 `project.yml` 里取出所有 `sources:` 底下的 `- path:`。
    ///
    /// **不写死一份根目录名单** —— 写死的那份会和 project.yml 各自漂，
    /// 而漂掉的那天没有任何读数会报警：新加的那个根从此不被看住，尺子照样全绿。
    static func sourcePaths(inProjectYAML text: String) -> [String] {
        var paths: [String] = []
        var sourcesIndent: Int? = nil
        for rawLine in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(rawLine)
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.isEmpty || trimmed.hasPrefix("#") { continue }
            let indent = line.prefix(while: { $0 == " " }).count

            if trimmed == "sources:" { sourcesIndent = indent; continue }
            guard let open = sourcesIndent else { continue }
            // 回到 `sources:` 同级或更外层 ⇒ 这个块结束了。
            if indent <= open { sourcesIndent = nil; continue }
            // 只收**块内第一层**的 `- path:`。`excludes:` 底下那些缩进更深，
            // 而且不带 `path:` 前缀，天然被这两个条件挡掉。
            guard trimmed.hasPrefix("- path:") else { continue }
            let value = trimmed.dropFirst("- path:".count)
                .trimmingCharacters(in: .whitespaces)
                .trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
            if !value.isEmpty && !paths.contains(value) { paths.append(value) }
        }
        return paths
    }

    /// 从 pbxproj 里取出所有 `path = xxx.swift`。带空格的名字在 pbxproj 里是带引号的。
    static func swiftBasenames(inPbxproj text: String) -> Set<String> {
        var found: Set<String> = []
        for piece in text.components(separatedBy: "path = ").dropFirst() {
            guard let end = piece.firstIndex(of: ";") else { continue }
            let value = String(piece[piece.startIndex..<end])
                .trimmingCharacters(in: .whitespaces)
                .trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
            if value.hasSuffix(".swift") { found.insert(value) }
        }
        return found
    }

    // MARK: - 读现场

    static func swiftBasenamesInPbxproj(root: URL) throws -> Set<String> {
        let url = root.appendingPathComponent("PendingCrew.xcodeproj/project.pbxproj")
        guard let text = try? String(contentsOf: url, encoding: .utf8) else {
            throw XCTSkip("读不到 \(url.path)（不在开发机上跑）")
        }
        return swiftBasenames(inPbxproj: text)
    }

    static func swiftBasenamesOnDisk(root: URL) throws -> Set<String> {
        let yamlURL = root.appendingPathComponent("project.yml")
        guard let yaml = try? String(contentsOf: yamlURL, encoding: .utf8) else {
            throw XCTSkip("读不到 \(yamlURL.path)（不在开发机上跑）")
        }
        let fm = FileManager.default
        var found: Set<String> = []
        for rel in sourcePaths(inProjectYAML: yaml) {
            let url = root.appendingPathComponent(rel)
            var isDir: ObjCBool = false
            guard fm.fileExists(atPath: url.path, isDirectory: &isDir) else { continue }
            if !isDir.boolValue {
                if url.pathExtension == "swift" { found.insert(url.lastPathComponent) }
                continue
            }
            guard let walker = fm.enumerator(at: url, includingPropertiesForKeys: nil,
                                             options: [.skipsHiddenFiles]) else { continue }
            for case let file as URL in walker where file.pathExtension == "swift" {
                // `project.yml` 明确 excludes 掉 Fixtures（那份数据不入 git）——
                // 这里跟着排掉，否则取过 fixture 的机器会假红。
                if file.pathComponents.contains("Fixtures") { continue }
                found.insert(file.lastPathComponent)
            }
        }
        return found
    }

    static func repoRoot() throws -> URL {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()   // PendingCrewTests
            .deletingLastPathComponent()   // Tests
            .deletingLastPathComponent()   // 仓库根
        guard FileManager.default.fileExists(atPath: root.appendingPathComponent("project.yml").path) else {
            throw XCTSkip("\(root.path) 下没有 project.yml（不在开发机上跑）")
        }
        return root
    }
}
