// SQLite knowledge API. Mirrors crewd-proto team_data; attributes remain lossless.
import Foundation

struct CrewdDataContext: Codable, Equatable, Hashable {
    var dbId: String
    var authorityId: String
    var restoreEpoch: String
    var crewId: String
    var namespace: String
    var generation: String
}

struct CrewdDataRef: Codable, Equatable {
    var namespace: String
    var kind: String
    var id: String
}

struct CrewdDataSource: Codable, Equatable {
    var reference: CrewdDataRefState?
    var atRevision: String?
    var attributes: CrewdValue
}

struct CrewdDataProvenance: Codable, Equatable {
    var createdBy: CrewdValue
    var updatedBy: CrewdValue
    var createdAt: String
    var updatedAt: String
    var sources: [CrewdDataSource]
    var attributes: CrewdValue
}

struct CrewdDataPlacement: Codable, Equatable {
    var id: String
    var block: CrewdDataRefState
    var atRevision: String?
    var layout: CrewdValue
}

enum CrewdDataCell: Codable, Equatable {
    case null
    case integer(value: String)
    case real(value: Double)
    case text(value: String)
    case blobHex(value: String)
    private enum Keys: String, CodingKey { case type, value }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .type) {
        case "null": self = .null
        case "integer": self = .integer(value: try c.decode(String.self, forKey: .value))
        case "real": self = .real(value: try c.decode(Double.self, forKey: .value))
        case "text": self = .text(value: try c.decode(String.self, forKey: .value))
        case "blobHex": self = .blobHex(value: try c.decode(String.self, forKey: .value))
        default: throw DecodingError.dataCorruptedError(forKey: .type, in: c, debugDescription: "Unknown knowledge discriminator")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case .null:
            try c.encode("null", forKey: .type)
        case let .integer(value):
            try c.encode("integer", forKey: .type)
            try c.encode(value, forKey: .value)
        case let .real(value):
            try c.encode("real", forKey: .type)
            try c.encode(value, forKey: .value)
        case let .text(value):
            try c.encode("text", forKey: .type)
            try c.encode(value, forKey: .value)
        case let .blobHex(value):
            try c.encode("blobHex", forKey: .type)
            try c.encode(value, forKey: .value)
        }
    }
}

struct CrewdDataSql: Codable, Equatable {
    var sql: String
    var params: [CrewdDataCell]
}

enum CrewdDataBindingSource: Codable, Equatable {
    case query(statement: CrewdDataSql)
    case object(reference: CrewdDataRefState, atRevision: String?)
    case objects(objectType: String, limit: UInt32)
    private enum Keys: String, CodingKey { case atRevision, kind, limit, objectType, reference, statement }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "query": self = .query(statement: try c.decode(CrewdDataSql.self, forKey: .statement))
        case "object": self = .object(reference: try c.decode(CrewdDataRefState.self, forKey: .reference), atRevision: try c.decodeIfPresent(String.self, forKey: .atRevision))
        case "objects": self = .objects(objectType: try c.decode(String.self, forKey: .objectType), limit: try c.decode(UInt32.self, forKey: .limit))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unknown knowledge discriminator")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case let .query(statement):
            try c.encode("query", forKey: .kind)
            try c.encode(statement, forKey: .statement)
        case let .object(reference, atRevision):
            try c.encode("object", forKey: .kind)
            try c.encode(reference, forKey: .reference)
            try c.encodeIfPresent(atRevision, forKey: .atRevision)
        case let .objects(objectType, limit):
            try c.encode("objects", forKey: .kind)
            try c.encode(objectType, forKey: .objectType)
            try c.encode(limit, forKey: .limit)
        }
    }
}

struct CrewdDataBinding: Codable, Equatable {
    var id: String
    var source: CrewdDataBindingSource
    var attributes: CrewdValue
}

enum CrewdDataViewDefinition: Codable, Equatable {
    case document(placements: [CrewdDataPlacement], layout: CrewdValue)
    case web(renderer: String, rendererVersion: String, html: String, css: String, javascript: String, bindings: [CrewdDataBinding], requestedCapabilities: [String], attributes: CrewdValue)
    private enum Keys: String, CodingKey { case attributes, bindings, css, html, javascript, kind, layout, placements, renderer, rendererVersion, requestedCapabilities }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "document": self = .document(placements: try c.decode([CrewdDataPlacement].self, forKey: .placements), layout: try c.decode(CrewdValue.self, forKey: .layout))
        case "web": self = .web(renderer: try c.decode(String.self, forKey: .renderer), rendererVersion: try c.decode(String.self, forKey: .rendererVersion), html: try c.decode(String.self, forKey: .html), css: try c.decode(String.self, forKey: .css), javascript: try c.decode(String.self, forKey: .javascript), bindings: try c.decode([CrewdDataBinding].self, forKey: .bindings), requestedCapabilities: try c.decode([String].self, forKey: .requestedCapabilities), attributes: try c.decode(CrewdValue.self, forKey: .attributes))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unknown knowledge discriminator")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case let .document(placements, layout):
            try c.encode("document", forKey: .kind)
            try c.encode(placements, forKey: .placements)
            try c.encode(layout, forKey: .layout)
        case let .web(renderer, rendererVersion, html, css, javascript, bindings, requestedCapabilities, attributes):
            try c.encode("web", forKey: .kind)
            try c.encode(renderer, forKey: .renderer)
            try c.encode(rendererVersion, forKey: .rendererVersion)
            try c.encode(html, forKey: .html)
            try c.encode(css, forKey: .css)
            try c.encode(javascript, forKey: .javascript)
            try c.encode(bindings, forKey: .bindings)
            try c.encode(requestedCapabilities, forKey: .requestedCapabilities)
            try c.encode(attributes, forKey: .attributes)
        }
    }
}

enum CrewdDataObject: Codable, Equatable {
    case block(id: String, revision: String, contentType: String, content: CrewdValue, archived: Bool, provenance: CrewdDataProvenance)
    case view(id: String, revision: String, title: String, definition: CrewdDataViewDefinition, archived: Bool, provenance: CrewdDataProvenance)
    case relation(id: String, revision: String, subject: CrewdDataRefState, predicate: String, object: CrewdDataRefState, attributes: CrewdValue, archived: Bool, provenance: CrewdDataProvenance)
    private enum Keys: String, CodingKey { case archived, attributes, content, contentType, definition, id, kind, object, predicate, provenance, revision, subject, title }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "block": self = .block(id: try c.decode(String.self, forKey: .id), revision: try c.decode(String.self, forKey: .revision), contentType: try c.decode(String.self, forKey: .contentType), content: try c.decode(CrewdValue.self, forKey: .content), archived: try c.decode(Bool.self, forKey: .archived), provenance: try c.decode(CrewdDataProvenance.self, forKey: .provenance))
        case "view": self = .view(id: try c.decode(String.self, forKey: .id), revision: try c.decode(String.self, forKey: .revision), title: try c.decode(String.self, forKey: .title), definition: try c.decode(CrewdDataViewDefinition.self, forKey: .definition), archived: try c.decode(Bool.self, forKey: .archived), provenance: try c.decode(CrewdDataProvenance.self, forKey: .provenance))
        case "relation": self = .relation(id: try c.decode(String.self, forKey: .id), revision: try c.decode(String.self, forKey: .revision), subject: try c.decode(CrewdDataRefState.self, forKey: .subject), predicate: try c.decode(String.self, forKey: .predicate), object: try c.decode(CrewdDataRefState.self, forKey: .object), attributes: try c.decode(CrewdValue.self, forKey: .attributes), archived: try c.decode(Bool.self, forKey: .archived), provenance: try c.decode(CrewdDataProvenance.self, forKey: .provenance))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unknown knowledge discriminator")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case let .block(id, revision, contentType, content, archived, provenance):
            try c.encode("block", forKey: .kind)
            try c.encode(id, forKey: .id)
            try c.encode(revision, forKey: .revision)
            try c.encode(contentType, forKey: .contentType)
            try c.encode(content, forKey: .content)
            try c.encode(archived, forKey: .archived)
            try c.encode(provenance, forKey: .provenance)
        case let .view(id, revision, title, definition, archived, provenance):
            try c.encode("view", forKey: .kind)
            try c.encode(id, forKey: .id)
            try c.encode(revision, forKey: .revision)
            try c.encode(title, forKey: .title)
            try c.encode(definition, forKey: .definition)
            try c.encode(archived, forKey: .archived)
            try c.encode(provenance, forKey: .provenance)
        case let .relation(id, revision, subject, predicate, object, attributes, archived, provenance):
            try c.encode("relation", forKey: .kind)
            try c.encode(id, forKey: .id)
            try c.encode(revision, forKey: .revision)
            try c.encode(subject, forKey: .subject)
            try c.encode(predicate, forKey: .predicate)
            try c.encode(object, forKey: .object)
            try c.encode(attributes, forKey: .attributes)
            try c.encode(archived, forKey: .archived)
            try c.encode(provenance, forKey: .provenance)
        }
    }
}

enum CrewdDataEdit: Codable, Equatable {
    case createBlock(alias: String, contentType: String, content: CrewdValue, sources: [CrewdDataSourceInput], attributes: CrewdValue)
    case updateBlock(id: String, expectedRevision: String, contentType: String, content: CrewdValue, sources: [CrewdDataSourceInput]?, attributes: CrewdValue)
    case forkBlock(alias: String, source: CrewdDataRef, atRevision: String?, attributes: CrewdValue)
    case archiveBlock(id: String, expectedRevision: String, archived: Bool)
    case createView(alias: String, title: String, definition: CrewdDataViewDefinitionInput, sources: [CrewdDataSourceInput], attributes: CrewdValue)
    case updateView(id: String, expectedRevision: String, title: String, definition: CrewdDataViewDefinitionInput, sources: [CrewdDataSourceInput]?, attributes: CrewdValue)
    case archiveView(id: String, expectedRevision: String, archived: Bool)
    case createRelation(alias: String, subject: CrewdDataInputRef, predicate: String, object: CrewdDataInputRef, attributes: CrewdValue)
    case archiveRelation(id: String, expectedRevision: String, archived: Bool)
    case sql(statements: [CrewdDataSql])
    private enum Keys: String, CodingKey { case alias, archived, atRevision, attributes, content, contentType, definition, expectedRevision, id, kind, object, predicate, source, sources, statements, subject, title }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "createBlock": self = .createBlock(alias: try c.decode(String.self, forKey: .alias), contentType: try c.decode(String.self, forKey: .contentType), content: try c.decode(CrewdValue.self, forKey: .content), sources: try c.decode([CrewdDataSourceInput].self, forKey: .sources), attributes: try c.decode(CrewdValue.self, forKey: .attributes))
        case "updateBlock": self = .updateBlock(id: try c.decode(String.self, forKey: .id), expectedRevision: try c.decode(String.self, forKey: .expectedRevision), contentType: try c.decode(String.self, forKey: .contentType), content: try c.decode(CrewdValue.self, forKey: .content), sources: try c.decodeIfPresent([CrewdDataSourceInput].self, forKey: .sources), attributes: try c.decode(CrewdValue.self, forKey: .attributes))
        case "forkBlock": self = .forkBlock(alias: try c.decode(String.self, forKey: .alias), source: try c.decode(CrewdDataRef.self, forKey: .source), atRevision: try c.decodeIfPresent(String.self, forKey: .atRevision), attributes: try c.decode(CrewdValue.self, forKey: .attributes))
        case "archiveBlock": self = .archiveBlock(id: try c.decode(String.self, forKey: .id), expectedRevision: try c.decode(String.self, forKey: .expectedRevision), archived: try c.decode(Bool.self, forKey: .archived))
        case "createView": self = .createView(alias: try c.decode(String.self, forKey: .alias), title: try c.decode(String.self, forKey: .title), definition: try c.decode(CrewdDataViewDefinitionInput.self, forKey: .definition), sources: try c.decode([CrewdDataSourceInput].self, forKey: .sources), attributes: try c.decode(CrewdValue.self, forKey: .attributes))
        case "updateView": self = .updateView(id: try c.decode(String.self, forKey: .id), expectedRevision: try c.decode(String.self, forKey: .expectedRevision), title: try c.decode(String.self, forKey: .title), definition: try c.decode(CrewdDataViewDefinitionInput.self, forKey: .definition), sources: try c.decodeIfPresent([CrewdDataSourceInput].self, forKey: .sources), attributes: try c.decode(CrewdValue.self, forKey: .attributes))
        case "archiveView": self = .archiveView(id: try c.decode(String.self, forKey: .id), expectedRevision: try c.decode(String.self, forKey: .expectedRevision), archived: try c.decode(Bool.self, forKey: .archived))
        case "createRelation": self = .createRelation(alias: try c.decode(String.self, forKey: .alias), subject: try c.decode(CrewdDataInputRef.self, forKey: .subject), predicate: try c.decode(String.self, forKey: .predicate), object: try c.decode(CrewdDataInputRef.self, forKey: .object), attributes: try c.decode(CrewdValue.self, forKey: .attributes))
        case "archiveRelation": self = .archiveRelation(id: try c.decode(String.self, forKey: .id), expectedRevision: try c.decode(String.self, forKey: .expectedRevision), archived: try c.decode(Bool.self, forKey: .archived))
        case "sql": self = .sql(statements: try c.decode([CrewdDataSql].self, forKey: .statements))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unknown knowledge discriminator")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case let .createBlock(alias, contentType, content, sources, attributes):
            try c.encode("createBlock", forKey: .kind)
            try c.encode(alias, forKey: .alias)
            try c.encode(contentType, forKey: .contentType)
            try c.encode(content, forKey: .content)
            try c.encode(sources, forKey: .sources)
            try c.encode(attributes, forKey: .attributes)
        case let .updateBlock(id, expectedRevision, contentType, content, sources, attributes):
            try c.encode("updateBlock", forKey: .kind)
            try c.encode(id, forKey: .id)
            try c.encode(expectedRevision, forKey: .expectedRevision)
            try c.encode(contentType, forKey: .contentType)
            try c.encode(content, forKey: .content)
            try c.encodeIfPresent(sources, forKey: .sources)
            try c.encode(attributes, forKey: .attributes)
        case let .forkBlock(alias, source, atRevision, attributes):
            try c.encode("forkBlock", forKey: .kind)
            try c.encode(alias, forKey: .alias)
            try c.encode(source, forKey: .source)
            try c.encodeIfPresent(atRevision, forKey: .atRevision)
            try c.encode(attributes, forKey: .attributes)
        case let .archiveBlock(id, expectedRevision, archived):
            try c.encode("archiveBlock", forKey: .kind)
            try c.encode(id, forKey: .id)
            try c.encode(expectedRevision, forKey: .expectedRevision)
            try c.encode(archived, forKey: .archived)
        case let .createView(alias, title, definition, sources, attributes):
            try c.encode("createView", forKey: .kind)
            try c.encode(alias, forKey: .alias)
            try c.encode(title, forKey: .title)
            try c.encode(definition, forKey: .definition)
            try c.encode(sources, forKey: .sources)
            try c.encode(attributes, forKey: .attributes)
        case let .updateView(id, expectedRevision, title, definition, sources, attributes):
            try c.encode("updateView", forKey: .kind)
            try c.encode(id, forKey: .id)
            try c.encode(expectedRevision, forKey: .expectedRevision)
            try c.encode(title, forKey: .title)
            try c.encode(definition, forKey: .definition)
            try c.encodeIfPresent(sources, forKey: .sources)
            try c.encode(attributes, forKey: .attributes)
        case let .archiveView(id, expectedRevision, archived):
            try c.encode("archiveView", forKey: .kind)
            try c.encode(id, forKey: .id)
            try c.encode(expectedRevision, forKey: .expectedRevision)
            try c.encode(archived, forKey: .archived)
        case let .createRelation(alias, subject, predicate, object, attributes):
            try c.encode("createRelation", forKey: .kind)
            try c.encode(alias, forKey: .alias)
            try c.encode(subject, forKey: .subject)
            try c.encode(predicate, forKey: .predicate)
            try c.encode(object, forKey: .object)
            try c.encode(attributes, forKey: .attributes)
        case let .archiveRelation(id, expectedRevision, archived):
            try c.encode("archiveRelation", forKey: .kind)
            try c.encode(id, forKey: .id)
            try c.encode(expectedRevision, forKey: .expectedRevision)
            try c.encode(archived, forKey: .archived)
        case let .sql(statements):
            try c.encode("sql", forKey: .kind)
            try c.encode(statements, forKey: .statements)
        }
    }
}

enum CrewdDataChange: Codable, Equatable {
    case apply(message: String, edits: [CrewdDataEdit])
    case restoreObject(reference: CrewdDataRef, revision: String, expectedRevision: String)
    private enum Keys: String, CodingKey { case kind, message, edits, reference, revision, expectedRevision }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "apply": self = .apply(message: try c.decode(String.self, forKey: .message), edits: try c.decode([CrewdDataEdit].self, forKey: .edits))
        case "restoreObject": self = .restoreObject(reference: try c.decode(CrewdDataRef.self, forKey: .reference), revision: try c.decode(String.self, forKey: .revision), expectedRevision: try c.decode(String.self, forKey: .expectedRevision))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unsupported knowledge change")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case .apply(let message, let edits):
            try c.encode("apply", forKey: .kind); try c.encode(message, forKey: .message); try c.encode(edits, forKey: .edits)
        case .restoreObject(let reference, let revision, let expectedRevision):
            try c.encode("restoreObject", forKey: .kind); try c.encode(reference, forKey: .reference)
            try c.encode(revision, forKey: .revision); try c.encode(expectedRevision, forKey: .expectedRevision)
        }
    }
}

struct CrewdDataSubmitParams: Codable, Equatable {
    var requestId: String
    var context: CrewdDataContext
    var change: CrewdDataChange
}

struct CrewdDataRequestGetParams: Codable, Equatable {
    var requestId: String
    var context: CrewdDataContext
}

struct CrewdDataObjectVersion: Codable, Equatable {
    var reference: CrewdDataRef
    var revision: String
}

enum CrewdDataPhase: String, Codable {
    case committed
    case rejected
    case conflicted
    case unknown
    case historical
}

struct CrewdDataChangeResult: Codable, Equatable {
    var requestId: String
    var phase: CrewdDataPhase
    var context: CrewdDataContext
    var beforeGeneration: String
    var afterGeneration: String
    var snapshotId: String?
    var created: [String: CrewdDataCreated]
    var changed: [CrewdDataObjectVersion]
    var conflicts: [CrewdDataConflict]
    var reason: String?
    var resolvedByRequest: String?
}

struct CrewdDataConflict: Codable, Equatable {
    var table: String
    var rowCount: String
    var schemaConflict: Bool
}

enum CrewdDataRequestLookup: Codable, Equatable {
    case known(value: CrewdDataChangeResult)
    case notFound
    case historical(value: CrewdDataChangeResult)
    private enum Keys: String, CodingKey { case result, status }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .status) {
        case "known": self = .known(value: try c.decode(CrewdDataChangeResult.self, forKey: .result))
        case "notFound": self = .notFound
        case "historical": self = .historical(value: try c.decode(CrewdDataChangeResult.self, forKey: .result))
        default: throw DecodingError.dataCorruptedError(forKey: .status, in: c, debugDescription: "Unknown knowledge discriminator")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case let .known(value):
            try c.encode("known", forKey: .status)
            try c.encode(value, forKey: .result)
        case .notFound:
            try c.encode("notFound", forKey: .status)
        case let .historical(value):
            try c.encode("historical", forKey: .status)
            try c.encode(value, forKey: .result)
        }
    }
}

struct CrewdDataReadParams: Codable, Equatable {
    var context: CrewdDataContext
    var atRevision: String?
}

struct CrewdDataObjectGetParams: Codable, Equatable {
    var context: CrewdDataContext
    var reference: CrewdDataRef
    var atRevision: String?
}

struct CrewdDataObjectListParams: Codable, Equatable {
    var context: CrewdDataContext
    var objectType: String
    var after: String?
    var limit: UInt32
    var atRevision: String?
    var includeArchived: Bool
}

struct CrewdDataQueryParams: Codable, Equatable {
    var context: CrewdDataContext
    var statement: CrewdDataSql
    var atRevision: String?
    var cursor: String?
    var limit: UInt32
}

struct CrewdDataRows: Codable, Equatable {
    var columns: [String]
    var rows: [[CrewdDataCell]]
    var truncated: Bool
    var next: String?
    var generation: String
    var stableOrder: Bool
}

struct CrewdDataHistoryParams: Codable, Equatable {
    var context: CrewdDataContext
    var reference: CrewdDataRef
    var after: String?
    var limit: UInt32
}

struct CrewdDataWorkspaceGetParams: Codable, Equatable {
    var crewId: String
}

struct CrewdDataWorkspaceInfo: Codable, Equatable {
    var context: CrewdDataContext
    var state: String
    var engineVersion: String
    var reason: String?
}

struct CrewdDataObjectPage: Codable, Equatable {
    var context: CrewdDataContext
    var generation: String
    var items: [CrewdDataObject]
    var next: String?
}

struct CrewdDataObjectDetail: Codable, Equatable {
    var context: CrewdDataContext
    var resolvedContext: CrewdDataContext
    var generation: String
    var object: CrewdDataObject
    var references: [CrewdDataResolvedRef]
    var affectedLiveViews: [CrewdDataRef]
    var affectedTruncated: Bool
}

enum CrewdDataRefState: Codable, Equatable {
    case visible(reference: CrewdDataRef)
    case unavailable
    private enum Keys: String, CodingKey { case reference, state }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .state) {
        case "visible": self = .visible(reference: try c.decode(CrewdDataRef.self, forKey: .reference))
        case "unavailable": self = .unavailable
        default: throw DecodingError.dataCorruptedError(forKey: .state, in: c, debugDescription: "Unknown knowledge discriminator")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case let .visible(reference):
            try c.encode("visible", forKey: .state)
            try c.encode(reference, forKey: .reference)
        case .unavailable:
            try c.encode("unavailable", forKey: .state)
        }
    }
}

struct CrewdDataColumn: Codable, Equatable {
    var name: String
    var declaredType: String
    var notNull: Bool
    var primaryKeyPosition: UInt32
    var defaultSql: String?
    var attributes: CrewdValue
}

struct CrewdDataSchemaObject: Codable, Equatable {
    var kind: String
    var name: String
    var createSql: String
    var columns: [CrewdDataColumn]
    var attributes: CrewdValue
}

struct CrewdDataSchemaResult: Codable, Equatable {
    var context: CrewdDataContext
    var generation: String
    var objects: [CrewdDataSchemaObject]
    var next: String?
}

struct CrewdDataSchemaParams: Codable, Equatable {
    var context: CrewdDataContext
    var atRevision: String?
    var after: String?
    var limit: UInt32
}

struct CrewdDataTableParams: Codable, Equatable {
    var context: CrewdDataContext
    var table: String
    var afterKey: [CrewdDataCell]?
    var atRevision: String?
    var limit: UInt32
}

struct CrewdDataTablePage: Codable, Equatable {
    var context: CrewdDataContext
    var generation: String
    var columns: [String]
    var rows: [[CrewdDataCell]]
    var nextKey: [CrewdDataCell]?
    var truncated: Bool
    var stableOrder: Bool
}

struct CrewdDataHistory: Codable, Equatable {
    var context: CrewdDataContext
    var items: [CrewdDataObject]
    var next: String?
}

enum CrewdDataInputRef: Codable, Equatable {
    case existing(reference: CrewdDataRef)
    case created(alias: String)
    private enum Keys: String, CodingKey { case alias, kind, reference }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "existing": self = .existing(reference: try c.decode(CrewdDataRef.self, forKey: .reference))
        case "created": self = .created(alias: try c.decode(String.self, forKey: .alias))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unknown knowledge discriminator")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case let .existing(reference):
            try c.encode("existing", forKey: .kind)
            try c.encode(reference, forKey: .reference)
        case let .created(alias):
            try c.encode("created", forKey: .kind)
            try c.encode(alias, forKey: .alias)
        }
    }
}

struct CrewdDataSourceInput: Codable, Equatable {
    var reference: CrewdDataInputRef?
    var atRevision: String?
    var attributes: CrewdValue
}

enum CrewdDataPlacementEdit: Codable, Equatable {
    case keep(id: String, layout: CrewdValue)
    case set(id: String?, alias: String?, block: CrewdDataInputRef, atRevision: String?, layout: CrewdValue)
    private enum Keys: String, CodingKey { case alias, atRevision, block, id, kind, layout }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "keep": self = .keep(id: try c.decode(String.self, forKey: .id), layout: try c.decode(CrewdValue.self, forKey: .layout))
        case "set": self = .set(id: try c.decodeIfPresent(String.self, forKey: .id), alias: try c.decodeIfPresent(String.self, forKey: .alias), block: try c.decode(CrewdDataInputRef.self, forKey: .block), atRevision: try c.decodeIfPresent(String.self, forKey: .atRevision), layout: try c.decode(CrewdValue.self, forKey: .layout))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unknown knowledge discriminator")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case let .keep(id, layout):
            try c.encode("keep", forKey: .kind)
            try c.encode(id, forKey: .id)
            try c.encode(layout, forKey: .layout)
        case let .set(id, alias, block, atRevision, layout):
            try c.encode("set", forKey: .kind)
            try c.encodeIfPresent(id, forKey: .id)
            try c.encodeIfPresent(alias, forKey: .alias)
            try c.encode(block, forKey: .block)
            try c.encodeIfPresent(atRevision, forKey: .atRevision)
            try c.encode(layout, forKey: .layout)
        }
    }
}

enum CrewdDataBindingInputSource: Codable, Equatable {
    case query(statement: CrewdDataSql)
    case object(reference: CrewdDataInputRef, atRevision: String?)
    case objects(objectType: String, limit: UInt32)
    private enum Keys: String, CodingKey { case atRevision, kind, limit, objectType, reference, statement }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "query": self = .query(statement: try c.decode(CrewdDataSql.self, forKey: .statement))
        case "object": self = .object(reference: try c.decode(CrewdDataInputRef.self, forKey: .reference), atRevision: try c.decodeIfPresent(String.self, forKey: .atRevision))
        case "objects": self = .objects(objectType: try c.decode(String.self, forKey: .objectType), limit: try c.decode(UInt32.self, forKey: .limit))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unknown knowledge discriminator")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case let .query(statement):
            try c.encode("query", forKey: .kind)
            try c.encode(statement, forKey: .statement)
        case let .object(reference, atRevision):
            try c.encode("object", forKey: .kind)
            try c.encode(reference, forKey: .reference)
            try c.encodeIfPresent(atRevision, forKey: .atRevision)
        case let .objects(objectType, limit):
            try c.encode("objects", forKey: .kind)
            try c.encode(objectType, forKey: .objectType)
            try c.encode(limit, forKey: .limit)
        }
    }
}

enum CrewdDataBindingEdit: Codable, Equatable {
    case keep(id: String)
    case set(id: String, source: CrewdDataBindingInputSource, attributes: CrewdValue)
    private enum Keys: String, CodingKey { case attributes, id, kind, source }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "keep": self = .keep(id: try c.decode(String.self, forKey: .id))
        case "set": self = .set(id: try c.decode(String.self, forKey: .id), source: try c.decode(CrewdDataBindingInputSource.self, forKey: .source), attributes: try c.decode(CrewdValue.self, forKey: .attributes))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unknown knowledge discriminator")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case let .keep(id):
            try c.encode("keep", forKey: .kind)
            try c.encode(id, forKey: .id)
        case let .set(id, source, attributes):
            try c.encode("set", forKey: .kind)
            try c.encode(id, forKey: .id)
            try c.encode(source, forKey: .source)
            try c.encode(attributes, forKey: .attributes)
        }
    }
}

enum CrewdDataViewDefinitionInput: Codable, Equatable {
    case document(placements: [CrewdDataPlacementEdit], layout: CrewdValue)
    case web(renderer: String, rendererVersion: String, html: String, css: String, javascript: String, bindings: [CrewdDataBindingEdit], requestedCapabilities: [String], attributes: CrewdValue)
    private enum Keys: String, CodingKey { case attributes, bindings, css, html, javascript, kind, layout, placements, renderer, rendererVersion, requestedCapabilities }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "document": self = .document(placements: try c.decode([CrewdDataPlacementEdit].self, forKey: .placements), layout: try c.decode(CrewdValue.self, forKey: .layout))
        case "web": self = .web(renderer: try c.decode(String.self, forKey: .renderer), rendererVersion: try c.decode(String.self, forKey: .rendererVersion), html: try c.decode(String.self, forKey: .html), css: try c.decode(String.self, forKey: .css), javascript: try c.decode(String.self, forKey: .javascript), bindings: try c.decode([CrewdDataBindingEdit].self, forKey: .bindings), requestedCapabilities: try c.decode([String].self, forKey: .requestedCapabilities), attributes: try c.decode(CrewdValue.self, forKey: .attributes))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unknown knowledge discriminator")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case let .document(placements, layout):
            try c.encode("document", forKey: .kind)
            try c.encode(placements, forKey: .placements)
            try c.encode(layout, forKey: .layout)
        case let .web(renderer, rendererVersion, html, css, javascript, bindings, requestedCapabilities, attributes):
            try c.encode("web", forKey: .kind)
            try c.encode(renderer, forKey: .renderer)
            try c.encode(rendererVersion, forKey: .rendererVersion)
            try c.encode(html, forKey: .html)
            try c.encode(css, forKey: .css)
            try c.encode(javascript, forKey: .javascript)
            try c.encode(bindings, forKey: .bindings)
            try c.encode(requestedCapabilities, forKey: .requestedCapabilities)
            try c.encode(attributes, forKey: .attributes)
        }
    }
}

enum CrewdDataCreated: Codable, Equatable {
    case object(object: CrewdDataObjectVersion)
    case placement(id: String, parentView: CrewdDataObjectVersion)
    private enum Keys: String, CodingKey { case id, kind, object, parentView }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "object": self = .object(object: try c.decode(CrewdDataObjectVersion.self, forKey: .object))
        case "placement": self = .placement(id: try c.decode(String.self, forKey: .id), parentView: try c.decode(CrewdDataObjectVersion.self, forKey: .parentView))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unknown knowledge discriminator")
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case let .object(object):
            try c.encode("object", forKey: .kind)
            try c.encode(object, forKey: .object)
        case let .placement(id, parentView):
            try c.encode("placement", forKey: .kind)
            try c.encode(id, forKey: .id)
            try c.encode(parentView, forKey: .parentView)
        }
    }
}

struct CrewdDataResolvedRef: Codable, Equatable {
    var anchor: String
    var target: CrewdDataRefState
    var commit: String?
    var revision: String?
    var live: Bool
    var complete: Bool
}

struct CrewdDataRequestGetResult: Codable, Equatable {
    var workspace: CrewdDataWorkspaceInfo
    var lookup: CrewdDataRequestLookup
}

struct CrewdDataPreviewParams: Codable { var context: CrewdDataContext; var statements: [CrewdDataSql] }
struct CrewdDataPreview: Codable, Equatable {
    var context: CrewdDataContext; var token: String; var statements: [CrewdDataSql]
    var affectedRows: UInt64; var schemaChanged: Bool
}
struct CrewdDataConfirmParams: Codable { var requestId: String; var context: CrewdDataContext; var token: String }
struct CrewdDataRestoreParams: Codable { var requestId: String; var context: CrewdDataContext; var snapshotId: String }
struct CrewdDataSnapshotsParams: Codable { var context: CrewdDataContext }
struct CrewdDataSnapshot: Codable, Equatable { var id: String; var createdAt: String; var generation: String }
struct CrewdDataSnapshots: Codable, Equatable { var context: CrewdDataContext; var items: [CrewdDataSnapshot] }

struct CrewdDataPreviewGetParams: Codable { var context: CrewdDataContext; var token: String }
