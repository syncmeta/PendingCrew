#if os(macOS)
import CryptoKit
import Darwin
import Foundation
import XCTest

/// The real Workspace supplies the RPC closure here. The Wire/Store mock suite
/// cannot catch a prerequisite being refused by that production closure.
@MainActor
final class CrewdManagedTableWorkspaceTests: XCTestCase {
    typealias Wire = CrewdManagedTableV3Wire

    func testOrdinaryCoreTableScopeReadsAndWritesTraverseActualWorkspace() async throws {
        try await withCore { fixture in
            let workspace = try await fixture.workspace(prepareJournal: true)
            let store = workspace.managedTablesV3
            let current: @MainActor () -> Bool = { workspace.client.readyForWork && !workspace.isRetired }
            // With the original production allowlist this throws the same
            // notSubmitted error observed in the installed organization tab.
            let scope = try await store.scope(stillCurrent: current)
            let empty = try await store.tables(scope, after: nil, includeArchived: false, current: current)
            XCTAssertTrue(empty.items.isEmpty)

            let title = "Workspace boundary acceptance"
            let purpose = "Synthetic private table; no agent or user data"
            let definition = try await store.definition(
                scope, crew: fixture.crew, excluding: nil, title: title,
                purpose: purpose, cursor: nil, current: current)
            XCTAssertEqual(definition.state, "complete")
            let token = try XCTUnwrap(definition.definitionCheckToken)
            let created = try await store.submit(
                .init(kind: "createTable", title: title, purpose: purpose,
                      fields: [.init(name: "Title", kind: .init(kind: .text), nullable: false)],
                      titleFieldIndex: 0, rowWritePolicy: .authorCaptainHuman,
                      definitionCheckToken: token, confirmNew: false),
                crew: fixture.crew, scope: scope, current: current)
            let tableID = created.receipt.effect.tableId
            XCTAssertEqual(created.receipt.operationKind, "createTable")
            XCTAssertFalse(created.historical)

            let table = try await store.table(tableID, scope: scope, current: current)
            let field = try XCTUnwrap(table.active.first)
            let added = try await store.submit(
                .init(kind: "addRow", tableId: table.id,
                      expectedSchemaRevision: table.schemaRevision,
                      patches: [.init(fieldId: field.id,
                                      change: .init(kind: "setScalar", value: .text("Actual Workspace row")))]),
                crew: fixture.crew, scope: scope, current: current)
            let rowID = try XCTUnwrap(added.receipt.effect.rowId)
            let page = try await store.rows(table.id, scope: scope, after: nil,
                                            includeArchived: false, current: current)
            XCTAssertEqual(page.items.map(\.id), [rowID])
            XCTAssertEqual(page.items.first?.scalar(field.id), .text("Actual Workspace row"))
            let history = try await store.history(.init(kind: "table", id: table.id),
                                                   scope: scope, cursor: nil, current: current)
            XCTAssertTrue(history.items.contains { $0.action == "createTable" })

            // Query the original UUID through the ordinary server, without
            // resubmitting the mutation or synthesizing another request ID.
            let lookup = try await fixture.control.call(Wire.lookup, .object([
                "scope": try CrewdKnowledgeStore.encode(scope.authority),
                "crewId": .string(fixture.crew), "requestId": .string(added.receipt.requestId),
            ]))
            XCTAssertEqual(lookup["status"].string, "known")
            XCTAssertEqual(lookup["receipt"]["requestId"].string, added.receipt.requestId)
            XCTAssertEqual(lookup["receipt"]["effect"]["rowId"].string, rowID)
            try fixture.save("table-roundtrip.json", .object([
                "tableId": .string(tableID), "rowId": .string(rowID),
                "createRequestId": .string(created.receipt.requestId),
                "addRequestId": .string(added.receipt.requestId),
                "originalLookup": lookup,
            ]))
        }
    }

    func testScopeStillRequiresTheActualWorkspaceJournal() async throws {
        try await withCore { fixture in
            let workspace = try await fixture.workspace(prepareJournal: false)
            XCTAssertTrue(workspace.client.readyForWork)
            XCTAssertTrue(workspace.managedTablesV3.supported)
            XCTAssertFalse(workspace.originalJournalReady)
            do {
                _ = try await workspace.managedTablesV3.scope(stillCurrent: { true })
                XCTFail("A connected client alone must not grant the table journal scope")
            } catch {}

            await workspace.prepareOriginalJournal(root: fixture.journal)
            XCTAssertTrue(workspace.originalJournalReady, workspace.error ?? "journal unavailable")
            let scope = try await workspace.managedTablesV3.scope(stillCurrent: { true })
            XCTAssertTrue(workspace.managedTablesV3.owns(scope))
            let page = try await workspace.managedTablesV3.tables(
                scope, after: nil, includeArchived: false, current: { true })
            XCTAssertTrue(page.items.isEmpty)
        }
    }

    func testScopeDoesNotSurviveCrewChangeOrWorkspaceRetirement() async throws {
        try await withCore { fixture in
            let workspace = try await fixture.workspace(prepareJournal: true)
            let store = workspace.managedTablesV3
            let original = try await store.scope(stillCurrent: { true })
            let other = try await fixture.createCrew("Second private crew")
            workspace.selectCrew(other)
            XCTAssertFalse(store.owns(original))
            do {
                _ = try await store.tables(original, after: nil, includeArchived: false, current: { true })
                XCTFail("The old crew's scope must be rejected")
            } catch {}
            let replacement = try await store.scope(stillCurrent: { true })
            XCTAssertTrue(store.owns(replacement))
            workspace.retire()
            XCTAssertFalse(store.owns(replacement))
            do {
                _ = try await store.scope(stillCurrent: { true })
                XCTFail("A retired Workspace must not regain table authority")
            } catch {}
        }
    }

    private func withCore(_ body: (ManagedTableWorkspaceCoreFixture) async throws -> Void) async throws {
        let fixture = try await ManagedTableWorkspaceCoreFixture.start()
        do {
            try await body(fixture)
        } catch {
            let original = error
            do { try await fixture.close() }
            catch { XCTFail("Original fixture shutdown unconfirmed; retained at \(fixture.root.path): \(error)") }
            throw original
        }
        try await fixture.close()
    }
}

/// Only an explicitly supplied, hash-bound ordinary Core is executed. No model
/// programs, app launch, daily authority, provider discovery or fallback build.
@MainActor
private final class ManagedTableWorkspaceCoreFixture {
    let root: URL
    let data: URL
    let journal: URL
    let process: Process
    let control = CrewdClient()
    private var log: FileHandle?
    private var workspaces: [CrewdWorkspace] = []
    private var shutdownRequest: CrewdValue?
    private var shutdownSubmitted = false
    private(set) var crew = ""

    private init(root: URL, process: Process, log: FileHandle) {
        self.root = root
        self.data = root.appendingPathComponent("data")
        self.journal = root.appendingPathComponent("presentation")
        self.process = process
        self.log = log
    }

    static func start() async throws -> ManagedTableWorkspaceCoreFixture {
        let env = ProcessInfo.processInfo.environment
        guard let raw = env["PENDINGCREW_TABLE_WORKSPACE_CORE"],
              let expected = env["PENDINGCREW_TABLE_WORKSPACE_CORE_SHA256"] else {
            throw XCTSkip("Supply the frozen ordinary Core path and SHA256 for the actual Workspace integration gate")
        }
        let program = URL(fileURLWithPath: raw)
        guard raw.hasPrefix("/"), program.resolvingSymlinksInPath().path == raw,
              FileManager.default.isExecutableFile(atPath: raw),
              expected.count == 64, expected.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else {
            throw FixtureError.invalidInput
        }
        let actual = SHA256.hash(data: try Data(contentsOf: program)).map { String(format: "%02x", $0) }.joined()
        guard actual == expected else { throw FixtureError.invalidInput }
        let parent = URL(fileURLWithPath: "/private/tmp", isDirectory: true)
        let root = parent.appendingPathComponent("pcw-v3-workspace-" + UUID().uuidString.prefix(12))
        for child in [root, root.appendingPathComponent("data"), root.appendingPathComponent("home"),
                      root.appendingPathComponent("cache"), root.appendingPathComponent("work")] {
            try FileManager.default.createDirectory(at: child, withIntermediateDirectories: false,
                                                     attributes: [.posixPermissions: 0o700])
        }
        let logURL = root.appendingPathComponent("core.log")
        guard FileManager.default.createFile(atPath: logURL.path, contents: nil,
                                             attributes: [.posixPermissions: 0o600]) else { throw FixtureError.invalidInput }
        let log = try FileHandle(forWritingTo: logURL)
        let process = Process()
        process.executableURL = program
        process.arguments = ["--data-dir", root.appendingPathComponent("data").path, "serve", "--disable-claude"]
        // Omitted --codex is the ordinary CLI's Disabled choice. Keep provider
        // environment overrides absent, including when the XCTest host has them.
        process.environment = ["PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
                               "HOME": root.appendingPathComponent("home").path,
                               "XDG_CACHE_HOME": root.appendingPathComponent("cache").path,
                               "SHELL": "/usr/bin/false"]
        process.currentDirectoryURL = root.appendingPathComponent("work")
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = log
        process.standardError = log
        let fixture = ManagedTableWorkspaceCoreFixture(root: root, process: process, log: log)
        try fixture.save("input.json", .object(["program": .string(raw), "sha256": .string(actual),
                                                "modelExecutorsDisabled": .bool(true)]))
        try process.run()
        try fixture.save("original-process.json", .object(["pid": .integer(Int64(process.processIdentifier))]))
        let socket = fixture.data.appendingPathComponent("crewd.sock")
        for _ in 0..<400 {
            guard process.isRunning else { throw FixtureError.coreExited }
            if FileManager.default.fileExists(atPath: socket.path) { break }
            try await Task.sleep(nanoseconds: 25_000_000)
        }
        try await fixture.control.connect(path: socket.path)
        guard fixture.control.readyForWork,
              fixture.control.capabilities.contains(CrewdManagedTableV3Wire.capability) else { throw FixtureError.invalidInput }
        let claim = try await fixture.control.call("lifecycle.claim", .object([:]))
        guard claim["role"].string == "owner" else { throw FixtureError.invalidInput }
        fixture.crew = try await fixture.createCrew("Actual Workspace private integration")
        print("Actual table Workspace fixture: \(root.path)")
        return fixture
    }

    func createCrew(_ title: String) async throws -> String {
        let reply = try await control.call("crew.create", .object([
            "title": .string(title), "workingDirectory": .string(root.appendingPathComponent("work").path),
        ]))
        guard let id = reply["id"].string, !id.isEmpty else { throw FixtureError.invalidReply }
        return id
    }

    func workspace(prepareJournal: Bool) async throws -> CrewdWorkspace {
        let workspace = CrewdWorkspace(dataDirectory: data)
        workspaces.append(workspace)
        workspace.originalJournalRequired = true
        await workspace.connect()
        guard workspace.client.readyForWork else { throw FixtureError.connection(workspace.error) }
        workspace.selectCrew(crew)
        if prepareJournal {
            await workspace.prepareOriginalJournal(root: journal)
            guard workspace.originalJournalReady else { throw FixtureError.connection(workspace.error) }
        }
        return workspace
    }

    func save(_ name: String, _ value: CrewdValue) throws {
        let bytes = try CrewdOrganizationSearchWire.encoder.encode(value)
        let path = root.appendingPathComponent(name)
        guard !FileManager.default.fileExists(atPath: path.path) else { throw FixtureError.invalidInput }
        try bytes.write(to: path, options: .withoutOverwriting)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path.path)
        let handle = try FileHandle(forWritingTo: path)
        try handle.synchronize()
        try handle.close()
        let parent = Darwin.open(root.path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
        guard parent >= 0 else { throw FixtureError.invalidInput }
        defer { Darwin.close(parent) }
        guard fsync(parent) == 0 else { throw FixtureError.invalidInput }
    }

    func close() async throws {
        workspaces.forEach { $0.retire() }
        workspaces.removeAll()
        guard process.isRunning else { throw FixtureError.coreExited }
        if shutdownRequest == nil {
            let request: CrewdValue = .object([
                "requestId": .string(UUID().uuidString.lowercased()),
                "dbId": control.runtimeIdentity["dbId"],
                "runtimeBindingId": control.runtimeIdentity["runtimeBindingId"],
                "command": .object(["operation": .string("crewd.hibernate"),
                                     "params": .object(["deadlineSecs": .integer(0)])]),
            ])
            try save("original-hibernate-request.json", request)
            shutdownRequest = request
        }
        guard let request = shutdownRequest else { throw FixtureError.invalidReply }
        var receipt: CrewdValue
        if shutdownSubmitted {
            receipt = try await control.call("command.lookup", .object([
                "dbId": request["dbId"], "requestId": request["requestId"],
            ]))
        } else {
            // This durable marker precedes the sole mutation. Failure never
            // resets it and never creates/replays a second shutdown request.
            try save("hibernate-submit-intent.json", .object(["requestId": request["requestId"]]))
            shutdownSubmitted = true
            receipt = try await control.call("command.submit", request)
        }
        for _ in 0..<200 where receipt["phase"].string != "confirmed" {
            try await Task.sleep(nanoseconds: 25_000_000)
            receipt = try await control.call("command.lookup", .object([
                "dbId": request["dbId"], "requestId": request["requestId"],
            ]))
        }
        guard receipt["phase"].string == "confirmed", receipt["requestId"] == request["requestId"] else {
            throw FixtureError.unconfirmedShutdown
        }
        try save("original-hibernate-confirmed.json", receipt)
        control.retire()
        for _ in 0..<400 where process.isRunning { try await Task.sleep(nanoseconds: 25_000_000) }
        guard !process.isRunning, process.terminationReason == .exit, process.terminationStatus == 0,
              !FileManager.default.fileExists(atPath: data.appendingPathComponent("crewd.sock").path) else {
            throw FixtureError.unconfirmedShutdown
        }
        try save("shutdown-result.json", .object(["naturalExit": .integer(0), "socketGone": .bool(true),
                                                  "requestId": request["requestId"]]))
        try log?.close()
        log = nil
    }

    private enum FixtureError: Error {
        case invalidInput, invalidReply, coreExited, unconfirmedShutdown, connection(String?)
    }
}
#endif
