#if os(macOS)
import XCTest

/// Argument construction uses filesystem metadata only; no provider is executed.
final class CrewdExecutorConfigurationTests:XCTestCase {
    private var directory:URL {URL(fileURLWithPath:"/private/tmp/pcw-disabled-config-fixture")}
    private func assertDisabled(_ codex:String?,_ claude:String?,file:StaticString=#filePath,line:UInt=#line) {
        let args=CrewdCodexConfiguration.launchArguments(directory:directory,program:codex,claudeProgram:claude)
        XCTAssertTrue(args.contains("--disable-claude"),"\(args)",file:file,line:line)
        XCTAssertFalse(args.contains("--claude"),"\(args)",file:file,line:line)
        XCTAssertFalse(args.contains("--codex"),"\(args)",file:file,line:line)
    }
    func testMissingProgramsUseDisabledSemantics() {assertDisabled(nil,nil)}
    func testExistingIsolationSentinelsUseDisabledSemantics() {
        assertDisabled("/usr/bin/false","/usr/bin/false")
        assertDisabled("/bin/false","/bin/false")
    }
    func testUnavailableFilesUseDisabledSemantics() {
        assertDisabled("/private/tmp/pcw-disabled-no-such-codex","/private/tmp/pcw-disabled-no-such-claude")
    }
    func testExplicitExecutableSelectionRemainsExplicit() {
        #if PENDINGCREW_LOCAL_DEMO
        // The Demo ignores executable inputs; its existing storage gate remains separate.
        assertDisabled("/usr/bin/true","/usr/bin/true")
        #else
        let args=CrewdCodexConfiguration.launchArguments(directory:directory,program:"/usr/bin/true",claudeProgram:"/usr/bin/true")
        XCTAssertFalse(args.contains("--disable-claude"))
        XCTAssertEqual(args[args.firstIndex(of:"--claude")!+1],"/usr/bin/true")
        XCTAssertEqual(args[args.firstIndex(of:"--codex")!+1],"/usr/bin/true")
        #endif
    }
    func testChildEnvironmentCannotOverrideAppSelection() {
        let preserved = ["HOME":"/synthetic/home", "PATH":"/synthetic/tools:/usr/bin:/bin",
                         "PENDINGCREW_CODEX_PROGRAM":"/usr/bin/true", "PENDINGCREW_CLAUDE_PROGRAM":"/usr/bin/true",
                         "PENDINGCREW_ISOLATED_CREDENTIALS":"memory"]
        XCTAssertEqual(CrewdCodexConfiguration.childEnvironment(preserved),preserved)
        var inherited = preserved
        inherited["CREWD_CODEX"] = "/synthetic/foreign-codex"
        inherited["CREWD_CLAUDE"] = "/synthetic/foreign-claude"
        XCTAssertEqual(CrewdCodexConfiguration.childEnvironment(inherited),preserved)
        XCTAssertEqual(inherited["CREWD_CLAUDE"],"/synthetic/foreign-claude")
        let args=CrewdCodexConfiguration.launchArguments(directory:directory,
            program:CrewdCodexConfiguration.program(environment:inherited),
            claudeProgram:CrewdClaudeConfiguration.program(environment:inherited))
        #if PENDINGCREW_LOCAL_DEMO
        XCTAssertTrue(args.contains("--disable-claude"))
        XCTAssertFalse(args.contains("--codex"))
        #else
        XCTAssertEqual(args[args.firstIndex(of:"--claude")!+1],"/usr/bin/true")
        XCTAssertEqual(args[args.firstIndex(of:"--codex")!+1],"/usr/bin/true")
        #endif
    }
}
#endif
