import Foundation

@main struct NativeTablesFixture {
    @MainActor static func main() async throws {
        let root = URL(fileURLWithPath:"/private/tmp/pcw-table-fixture-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:root) }
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,authorityID:"db",
            databaseID:"db",grantID:nil,grantRevision:nil)
        let origin = CrewdKnowledgeStore.Origin(scope:scope,observation:UUID(),connection:UUID(),crew:"crew")
        let journal = try CrewdPresentationJournal(root:root,scope:scope)
        let definition = CrewdNativeTable.Definition(name:"交接\"事项",columns:[
            .init(name:"编号",type:.text,notNull:true),.init(name:"次数",type:.integer,notNull:false),.init(name:"说明",type:.text,notNull:false)])
        let create = try definition.createStatement()
        precondition(create.sql == "CREATE TABLE \"交接\"\"事项\" (\"编号\" TEXT NOT NULL PRIMARY KEY, \"次数\" INTEGER, \"说明\" TEXT) STRICT")
        let schema = CrewdDataSchemaObject(kind:"table",name:definition.name,createSql:create.sql,
            columns:definition.columns.enumerated().map { i, column in .init(name:column.name,declaredType:column.type.rawValue,
                notNull:column.notNull,primaryKeyPosition:i == 0 ? 1 : 0,defaultSql:nil,attributes:.null) },
            attributes:.object(["strict":.bool(true),"stablePrimaryKey":.bool(true),"nativeRowWrites":.bool(true)]))
        let model = CrewdNativeTable.Model(schema)!
        let decomposed = "e\u{301}"
        let original: [CrewdDataCell] = [.text(value:decomposed),.integer(value:"-9223372036854775808"),.null]
        let edited: [CrewdDataCell] = [original[0], original[1], .text(value:"é'; DELETE FROM x; --")]
        let insert = try model.statement(row:original,original:nil)
        let update = try model.statement(row:edited,original:original)
        precondition(update.sql.hasPrefix("UPDATE OR ABORT ") && !update.sql.contains("DELETE") && update.params == [edited[2]] + original)
        precondition(insert.sql.hasPrefix("INSERT OR ABORT INTO ") && insert.params == original && !CrewdNativeTable.exact(.text(value:"é"),original[0]))
        let readOnly = CrewdDataSchemaObject(kind:schema.kind,name:schema.name,createSql:schema.createSql,columns:schema.columns,
            attributes:.object(["strict":.bool(true),"stablePrimaryKey":.bool(true),"nativeRowWrites":.bool(false)]))
        precondition(CrewdNativeTable.Model(readOnly) == nil)
        do {
            _ = try model.statement(row:[.text(value:"é"),edited[1],edited[2]],original:original)
            preconditionFailure("canonical-equivalent key rewrite accepted")
        } catch {}
        let fake = Backend(schema:schema,create:create,rows:original)
        let store = CrewdKnowledgeStore(origin:{origin},rpc:{ method, params in try fake.call(method,params) },journal:{journal})
        await store.refresh(loadViews:false)
        let uninitialized = try await store.nativeTableCatalog()
        precondition(uninitialized.objects.isEmpty && uninitialized.generation == "0")
        let createIntent = try store.newNativeTable(definition,context:fake.context)
        let createReview = try await store.previewNativeTable(createIntent,stillCurrent:{true})
        let created = try await store.confirmNativeTable(createReview,stillCurrent:{true})
        precondition(created == .exact)
        await store.refresh(loadViews:false)
        let catalog = try await store.nativeTableCatalog()
        precondition(catalog.objects == [schema])
        fake.shortCatalog = true
        let shortCatalog = try await store.nativeTableCatalog()
        precondition(shortCatalog.objects == [schema] && shortCatalog.next == schema.name)
        let continuedCatalog = try await store.nativeTableCatalog(after:shortCatalog.next)
        precondition(continuedCatalog.objects.map(\.name) == ["𠀀下一张表"] && continuedCatalog.next == nil)
        fake.shortCatalog = false
        let page = try await store.nativeTablePage(schema,context:fake.context)
        precondition(page.stableOrder && page.rows.isEmpty)
        fake.shortTablePage = true
        let byteShort = try await store.nativeTablePage(schema,context:fake.context)
        precondition(byteShort.rows.count == 1 && byteShort.nextKey == [original[0]])
        let byteContinuation = try await store.nativeTablePage(schema,context:fake.context,after:byteShort.nextKey)
        precondition(byteContinuation.rows.first?.first == .text(value:"z") && byteContinuation.nextKey == nil)
        fake.shortTablePage = false

        let add = try store.saveNativeTableRow(model,row:original,original:nil,context:fake.context)
        let addPreview = try await store.previewNativeTable(add,stillCurrent:{true})
        fake.dropReply = true
        do { _ = try await store.confirmNativeTable(addPreview,stillCurrent:{true}); preconditionFailure("lost reply looked verified") } catch {}
        let prepared = try journal.unknownRecords()
        let originalID = prepared.first!.params["id"].string!.lowercased()
        precondition(prepared.count == 1 && originalID == fake.originalID)
        let reopenedJournal = try CrewdPresentationJournal(root:root,scope:scope)
        let reopened = CrewdKnowledgeStore(origin:{origin},rpc:{ try fake.call($0,$1) },journal:{reopenedJournal})
        await reopened.refresh(loadViews:false)
        precondition(reopened.hasBlockingPending)
        fake.missing = true
        let missing = try await reopened.lookupNativeTable(UUID(uuidString:originalID)!,stillCurrent:{true})
        precondition(missing == nil && reopened.hasBlockingPending)
        fake.missing = false
        let confirmedAdd = try await reopened.lookupNativeTable(UUID(uuidString:originalID)!,stillCurrent:{true})
        precondition(confirmedAdd == .exact)
        let afterRecovery = try journal.unknownRecords()
        precondition(afterRecovery.isEmpty && fake.calls.filter { $0 == "data.confirm" }.count == 2)
        await reopened.refresh(loadViews:false)

        let edit = try reopened.saveNativeTableRow(model,row:edited,original:original,context:fake.context)
        let editPreview = try await reopened.previewNativeTable(edit,stillCurrent:{true})
        let editedResult = try await reopened.confirmNativeTable(editPreview,stillCurrent:{true})
        precondition(editedResult == .exact && fake.rows == edited)
        precondition(fake.calls.filter { $0 == "data.request.get" }.count == 2)
        await reopened.refresh(loadViews:false)
        let reverted = try reopened.saveNativeTableRow(model,row:original,original:edited,context:fake.context)
        let reverting = try await reopened.previewNativeTable(reverted,stillCurrent:{true})
        fake.dropReply = true
        do { _ = try await reopened.confirmNativeTable(reverting,stillCurrent:{true}); preconditionFailure("lost reply looked verified") } catch {}
        let revertID = UUID(uuidString:fake.originalID)!
        fake.context.generation = String(Int(fake.context.generation)! + 1)
        fake.rows = edited
        await reopened.refresh(loadViews:false)
        let currentDiffers = try await reopened.lookupNativeTable(revertID,stillCurrent:{true})
        precondition(currentDiffers == .currentDiffers && fake.calls.filter { $0 == "data.confirm" }.count == 4)
        await reopened.refresh(loadViews:false)
        let next = try reopened.saveNativeTableRow(model,row:original,original:edited,context:fake.context)
        let nextReview = try await reopened.previewNativeTable(next,stillCurrent:{true})
        fake.dropReply = true
        do { _ = try await reopened.confirmNativeTable(nextReview,stillCurrent:{true}); preconditionFailure("lost reply looked verified") } catch {}
        let replacedID = UUID(uuidString:fake.originalID)!
        fake.context.generation = String(Int(fake.context.generation)! + 1)
        fake.currentSchema = "CREATE TABLE \"交接\"\"事项\" (replacement INTEGER PRIMARY KEY) STRICT"
        await reopened.refresh(loadViews:false)
        let replacement = try await reopened.lookupNativeTable(replacedID,stillCurrent:{true})
        precondition(replacement == .currentDiffers && fake.calls.filter { $0 == "data.confirm" }.count == 5)
        let finalPending = try reopenedJournal.unknownRecords()
        precondition(finalPending.isEmpty)
        print("PASS native typed table CREATE/Add/Edit, byte-short catalog, exact immutable key, original UUID recovery, and newer same-name schema replacement")
    }

    @MainActor private final class Backend {
        let schema: CrewdDataSchemaObject
        let create: CrewdDataSql
        var rows: [CrewdDataCell]
        var context = CrewdDataContext(dbId:"db",authorityId:"db",restoreEpoch:"initial",crewId:"crew",namespace:"sqlite",generation:"0")
        var initialized = false, hasRow = false, dropReply = false, missing = false
        var shortCatalog = false, shortTablePage = false
        var currentSchema: String?
        var calls: [String] = [], originalID = ""
        var staged: CrewdDataPreview?, last: CrewdDataChangeResult?
        init(schema:CrewdDataSchemaObject,create:CrewdDataSql,rows:[CrewdDataCell]) { self.schema=schema;self.create=create;self.rows=rows }
        var info: CrewdDataWorkspaceInfo { .init(context:context,state:initialized ? "ready" : "uninitialized",engineVersion:"SQLite",reason:nil) }
        func call(_ method:String,_ params:CrewdValue) throws -> CrewdValue {
            calls.append(method)
            switch method {
            case "data.workspace.get": return try CrewdKnowledgeStore.encode(info)
            case "data.schema.get":
                if shortCatalog && params["after"] != .null {
                    precondition(params["after"] == .string(schema.name))
                    var later = schema
                    later.name = "𠀀下一张表"
                    return try CrewdKnowledgeStore.encode(CrewdDataSchemaResult(context:context,generation:context.generation,objects:[later],next:nil))
                }
                return try CrewdKnowledgeStore.encode(CrewdDataSchemaResult(context:context,generation:context.generation,objects:[schema],next:shortCatalog ? schema.name:nil))
            case "data.table.get":
                if shortTablePage {
                    let input = try CrewdKnowledgeStore.decode(params,CrewdDataTableParams.self)
                    let first = input.afterKey == nil
                    var current = rows
                    if !first {
                        precondition(input.afterKey == [rows[0]])
                        current[0] = .text(value:"z")
                    }
                    return try CrewdKnowledgeStore.encode(CrewdDataTablePage(context:context,generation:context.generation,
                        columns:schema.columns.map(\.name),rows:[current],nextKey:first ? [current[0]] : nil,truncated:first,stableOrder:true))
                }
                return try CrewdKnowledgeStore.encode(CrewdDataTablePage(context:context,generation:context.generation,columns:schema.columns.map(\.name),
                    rows:hasRow ? [rows] : [],nextKey:nil,truncated:false,stableOrder:true))
            case "data.preview":
                let input = try CrewdKnowledgeStore.decode(params,CrewdDataPreviewParams.self)
                precondition(input.context == context && input.statements.count == 1)
                let creating = input.statements == [create]
                let result = CrewdDataPreview(context:context,token:UUID().uuidString.lowercased(),statements:input.statements,
                    affectedRows:creating ? 0 : 1,schemaChanged:creating)
                staged=result
                return try CrewdKnowledgeStore.encode(result)
            case "data.confirm":
                let input = try CrewdKnowledgeStore.decode(params,CrewdDataConfirmParams.self)
                precondition(input.context == context && input.token == staged?.token)
                let before = context.generation
                context.generation = String(Int(before)! + 1)
                if let staged, !staged.schemaChanged {
                    if !hasRow { rows = staged.statements[0].params; hasRow = true }
                    else { rows[2] = staged.statements[0].params[0] }
                }
                initialized=true
                originalID=input.requestId
                let result = CrewdDataChangeResult(requestId:input.requestId,phase:.committed,context:context,beforeGeneration:before,
                    afterGeneration:context.generation,snapshotId:UUID().uuidString.lowercased(),created:[:],changed:[],conflicts:[],reason:nil,resolvedByRequest:nil)
                last=result;staged=nil
                if dropReply { dropReply = false; throw CrewdClientError(message:"lost") }
                return try CrewdKnowledgeStore.encode(result)
            case "data.request.get":
                precondition(params["requestId"] == .string(originalID) && params["context"]["generation"] == .string(last!.beforeGeneration))
                return try CrewdKnowledgeStore.encode(CrewdDataRequestGetResult(workspace:info,lookup:missing ? .notFound : .known(value:last!)))
            case "data.query":
                let input = try CrewdKnowledgeStore.decode(params,CrewdDataQueryParams.self)
                precondition(input.context == context && input.limit == 2)
                let sql = input.statement.sql.contains("sqlite_schema")
                if !sql {
                    precondition(currentSchema == nil, "old columns queried against replacement table")
                    precondition(CrewdNativeTable.exact(input.statement.params[0],rows[0]))
                }
                let result = CrewdDataRows(columns:sql ? ["sql"] : schema.columns.map(\.name),
                    rows:sql ? [[.text(value:currentSchema ?? create.sql)]] : [rows],truncated:false,next:nil,generation:context.generation,stableOrder:false)
                return try CrewdKnowledgeStore.encode(result)
            default: preconditionFailure(method)
            }
        }
    }
}
