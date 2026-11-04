import Foundation
import Darwin

@main struct CrewdCodexNetworkRouteFixture {
    static func main() throws {
        let codex = "/usr/bin/true"
        let data = URL(fileURLWithPath:"/private/tmp/pcw-codex-route-fixture")
        let mode = CommandLine.arguments.dropFirst().first
        if mode == "observe-system" {
            let route = CrewdCodexConfiguration.networkRoute(program:codex,environment:[:],useSystemProxy:true)
            // The actual configured address and system exceptions stay local.
            precondition(route.status == .systemHTTPS,"Current static loopback route is not eligible")
            print("PASS current macOS static loopback HTTPS address is eligible for explicit Codex-only mode")
            return
        }
        func args(_ route: CrewdCodexConfiguration.NetworkRoute, program: String? = "/usr/bin/true", claude: String? = nil) -> [String] {
            CrewdCodexConfiguration.launchArguments(directory:data,program:program,claudeProgram:claude,networkRoute:route)
        }
        let system: [String:Any] = [
            "HTTPSEnable":1, "HTTPSProxy":"127.0.0.2", "HTTPSPort":4_321,
            "HTTPEnable":1, "HTTPProxy":"different.invalid", "SOCKSEnable":1,
            "ExceptionsList":["*.example.test","169.254/16","private.example"], "ExcludeSimpleHostnames":1
        ]
        func qualify(_ system: [String:Any]?) -> CrewdCodexConfiguration.NetworkRoute {
            CrewdCodexConfiguration.resolveNetworkRoute(enabled:true,codexEnabled:true,environment:[:],readSystem:{system})
        }
        if mode == "emit-argv", CommandLine.arguments.count == 3 || CommandLine.arguments.count == 4 {
            let source = CommandLine.arguments.dropFirst(3).first ?? "synthetic"
            precondition(["synthetic","system"].contains(source))
            let route = source == "system"
                ? CrewdCodexConfiguration.networkRoute(program:codex,environment:[:],useSystemProxy:true)
                : qualify(system)
            precondition(route.status == .systemHTTPS)
            let path = CommandLine.arguments[2]
            let parent = URL(fileURLWithPath:path).deletingLastPathComponent().path
            var info = stat()
            let resolved = realpath(parent,nil)
            defer { free(resolved) }
            guard path.hasPrefix("/private/tmp/"), !path.split(separator:"/").contains(".."),
                  let resolved, String(cString:resolved) == parent,
                  lstat(parent,&info) == 0, (info.st_mode & S_IFMT) == S_IFDIR,
                  info.st_uid == getuid(), info.st_mode & 0o077 == 0 else { throw CocoaError(.fileWriteNoPermission) }
            let fd = open(path,O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW,mode_t(0o600))
            guard fd >= 0 else { throw CocoaError(.fileWriteUnknown) }
            let file = FileHandle(fileDescriptor:fd,closeOnDealloc:true)
            try file.write(contentsOf:JSONEncoder().encode(args(route)))
            try file.synchronize()
            return
        }
        func replace(_ key: String, _ value: Any) -> [String:Any] {
            var copy = system
            copy[key] = value
            return copy
        }
        let defaultRoute = CrewdCodexConfiguration.resolveNetworkRoute(enabled:false,codexEnabled:true,environment:[:]) {
            preconditionFailure("Default must not read system settings")
        }
        precondition(defaultRoute.status == .inherited)
        let baseline = CrewdCodexConfiguration.launchArguments(directory:data,program:codex,claudeProgram:nil)
        let disabledArgs = CrewdCodexConfiguration.launchArguments(directory:data,program:nil,claudeProgram:nil)
        precondition(args(defaultRoute) == baseline)
        for name in ["HTTP_PROXY","http_proxy","HTTPS_PROXY","https_proxy","All_Proxy","NO_PROXY","no_proxy","Ws_Proxy","WSS_PROXY"] {
            let route = CrewdCodexConfiguration.resolveNetworkRoute(enabled:true,codexEnabled:true,environment:[name:""]) {
                preconditionFailure("Existing explicit policy must not read system settings")
            }
            precondition(route.status == .explicitEnvironment && args(route) == baseline)
        }
        let disabled = CrewdCodexConfiguration.resolveNetworkRoute(enabled:true,codexEnabled:false,environment:[:]) {
            preconditionFailure("Disabled Codex must not read system settings")
        }
        precondition(disabled.status == .noCodex)
        let valid = qualify(system)
        precondition(valid.status == .systemHTTPS)
        let expected = ["--codex-env","HTTPS_PROXY=http://127.0.0.2:4321","--codex-env","https_proxy=http://127.0.0.2:4321",
            "--codex-env","NO_PROXY=localhost,127.0.0.1,::1","--codex-env","no_proxy=localhost,127.0.0.1,::1"]
        precondition(args(valid) == baseline + expected)
        let mirror = replace("__SCOPED__",["test0":system,"test1":system])
        precondition(qualify(mirror).status == .systemHTTPS && args(qualify(mirror)) == baseline + expected)
        for scoped in [replace("HTTPSPort",9999),replace("HTTPSProxy","127.0.0.1"),replace("HTTPSEnable",0),
                       replace("ProxyAutoDiscoveryEnable",1),replace("HTTPSUser","fixture")] {
            let different = qualify(replace("__SCOPED__",["test0":system,"test1":scoped]))
            precondition(different.status == .unsupportedPolicy && args(different) == disabledArgs)
        }
        precondition(args(valid,program:"/usr/bin/false") == args(defaultRoute,program:"/usr/bin/false"))
        precondition(args(valid,program:nil) == args(defaultRoute,program:nil))
        let withClaude = args(valid,claude:"/usr/bin/true")
        precondition(withClaude == args(defaultRoute,claude:"/usr/bin/true") + expected)
        precondition(!withClaude.contains("--claude-env") && !withClaude.contains(where:{ $0.contains("different.invalid") || $0.contains("example.test") }))
        let inherited = ["PATH":"/usr/bin","NO_PROXY":"original-bypass","CREWD_CODEX":"never","CREWD_CLAUDE":"never","OTHER_SETTING":"retained"]
        precondition(CrewdCodexConfiguration.childEnvironment(inherited) == ["PATH":"/usr/bin","NO_PROXY":"original-bypass","OTHER_SETTING":"retained"])
        for host in ["::1","[::1]","0:0:0:0:0:0:0:1"] {
            let route = qualify(replace("HTTPSProxy",host))
            precondition(route.status == .systemHTTPS && args(route).contains("HTTPS_PROXY=http://[::1]:4321"))
        }
        for host in ["localhost","10.0.0.1","::2","127.1","127.00.0.1","::ffff:127.0.0.1","user@127.0.0.1",
                     "https://127.0.0.1","[::1]:1","::1%lo0","::1\u{0}unsafe","127.0.0.1\n"] {
            let route = qualify(replace("HTTPSProxy",host))
            precondition(route.status == .unsupportedEndpoint && args(route) == disabledArgs,"unexpected synthetic host decision: \(host.debugDescription)")
        }
        for port: Any in [0,-1,65_536,"4321",12.5,true] {
            let route = qualify(replace("HTTPSPort",port))
            precondition(route.status == .unsupportedEndpoint && args(route) == disabledArgs,"unexpected synthetic port decision: \(port)")
        }
        for (key,value) in [("ProxyAutoConfigEnable",1 as Any),("ProxyAutoDiscoveryEnable",1),
                            ("__SCOPED__",["interface":[:]]),("__SUPPLEMENTAL__",["routes":[]]),
                            ("HTTPSUser","someone"),("HTTPSUser",12)] {
            let route = qualify(replace(key,value))
            precondition(route.status == .unsupportedPolicy && args(route) == disabledArgs)
        }
        precondition(qualify(nil).status == .settingsUnavailable && args(qualify(nil)) == disabledArgs)
        let unavailable = qualify(replace("HTTPSEnable",0))
        precondition(unavailable.status == .noStaticHTTPS && args(unavailable) == disabledArgs)
        print("PASS default off, explicit precedence, static loopback IPv4 and IPv6, fixed local bypass, and unsupported route refusal")
    }
}
