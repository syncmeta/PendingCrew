import CryptoKit
import Foundation

/// The V3 table contract is separate from both old closed table codecs.
enum CrewdManagedTableV3Wire {
  /// Shared by the real editor and held-response state regression.
  struct EditorRequests {
    var ticket = UUID()
    var busy = false
    private(set) var definitionVersion = UUID()
    mutating func begin() -> UUID {
      ticket = UUID()
      busy = true
      return ticket
    }
    func owns(_ value: UUID) -> Bool { ticket == value }
    mutating func finish(_ value: UUID) { if owns(value) { busy = false } }
    mutating func cancel() {
      ticket = UUID()
      busy = false
    }
    mutating func definitionChanged() { definitionVersion = UUID() }
    mutating func handoff(from value: UUID) -> UUID? {
      guard owns(value) else { return nil }
      return begin()
    }
  }
  static func titleAfterRemoval(selected: UUID?, removed: UUID, order: [UUID]) -> UUID? {
    guard let selected, selected != removed, order.contains(selected) else { return nil }
    return selected
  }
  typealias V2 = CrewdManagedTableV2Wire
  typealias Scope = V2.Scope
  typealias Policy = V2.Policy
  typealias Author = V2.Author
  typealias Target = V2.Target
  typealias Constraint = V2.Constraint
  typealias LinkWrite = V2.LinkWrite
  typealias LinkCell = V2.LinkCell
  typealias Intent = CrewdManagedV2Intent.Value
  static let capability = "managedTables.v3", observation = "managedTable.v3.observation"
  static let tableGet = "managedTable.v3.get", tableList = "managedTable.v3.list",
    rowGet = "managedTable.v3.row.get", rows = "managedTable.v3.rows",
    rowLinks = "managedTable.v3.row.links"
  static let submit = "managedTable.v3.submit", lookup = "managedTable.v3.request.get",
    defineCheck = "managedTable.v3.defineCheck", history = "managedTable.v3.history",
    historyValue = "managedTable.v3.history.value"
  static let methods = [
    tableGet, tableList, rowGet, rows, rowLinks, submit, lookup, defineCheck, history, historyValue,
  ]
  static var readMethods: [String] { methods.filter { $0 != submit } }
  static let bytePage = 262_144, historyBytes = 32_768, chunkBytes = 4_096
  enum Failure: LocalizedError {
    case invalid, unloaded(String), required(String), bounds(String)
    var errorDescription: String? {
      switch self {
      case .invalid: "表格内容或原请求证明不一致。"
      case .unloaded(let f): "请先读取关联列「" + f + "」的原值。"
      case .required(let f): "请明确填写必填列「" + f + "」。"
      case .bounds(let f): "请核对「" + f + "」的类型或长度。"
      }
    }
  }
  enum NewPolicy: String, Codable, CaseIterable, Identifiable {
    case authorCaptainHuman, ownerCrewWriters
    var id: Self { self }
    var title: String { Policy(rawValue: rawValue)!.title }
  }
  enum Tone: String, Codable, CaseIterable, Identifiable {
    case neutral, good, attention, bad, info
    var id: Self { self }
    var title: String {
      switch self {
      case .neutral: "中性"
      case .good: "正向"
      case .attention: "注意"
      case .bad: "风险"
      case .info: "信息"
      }
    }
  }
  enum TypeName: String, Codable, CaseIterable, Identifiable {
    case text, longText, integer, real, decimal, singleSelect, date, boolean, url, person, link
    var id: Self { self }
    var title: String {
      switch self {
      case .text: "文本"
      case .longText: "长文本"
      case .integer: "整数"
      case .real: "浮点数"
      case .decimal: "精确小数"
      case .singleSelect: "单选"
      case .date: "日期"
      case .boolean: "是否"
      case .url: "网址"
      case .person: "人员"
      case .link: "对象关联"
      }
    }
    var isLink: Bool { self == .link || self == .person }
  }
  static func bytes(_ a: String, _ b: String) -> Bool { a.utf8.elementsEqual(b.utf8) }
  static func exact<T: Encodable>(_ a: T, _ b: T) -> Bool { V2.exact(a, b) }
  static func number(_ s: String) -> UInt64? { V2.number(s) }
  static func positive(_ s: String) -> Bool { V2.positive(s) }
  static func label(_ s: String, limit: Int) -> Bool { V2.label(s, limit: limit) }
  static func objectID(_ s: String, _ prefix: Character) -> Bool {
    CrewdManagedDirectoryWire.objectNumber(s, prefix: prefix) != nil
  }
  static func text(_ s: String, bounds: TextBounds) -> Bool {
    s.utf8.count <= bounds.maxUtf8Bytes
      && (bounds.maxUnicodeScalars.map({ s.unicodeScalars.count <= $0 }) ?? true)
  }
  static func decimal(_ s: String) -> Bool {
    let b = Array(s.utf8)
    guard !b.isEmpty, b.count <= 59 else { return false }
    let negative = b.first == 45
    let body = Array(b.dropFirst(negative ? 1 : 0))
    guard !body.isEmpty else { return false }
    let pieces = body.split(separator: 46, omittingEmptySubsequences: false)
    guard (1...2).contains(pieces.count), !pieces[0].isEmpty,
      pieces.allSatisfy({ $0.allSatisfy { (48...57).contains($0) } }),
      pieces[0].count == 1 || pieces[0].first != 48
    else { return false }
    if pieces.count == 2 {
      guard !pieces[1].isEmpty, pieces[1].count <= 18, pieces[1].last != 48 else { return false }
    }
    let coefficient = pieces.flatMap({ Array($0) }).drop(while: { $0 == 48 })
    guard coefficient.count <= 38 else { return false }
    return !coefficient.isEmpty || (!negative && s == "0")
  }
  static func date(_ s: String) -> Bool {
    let b = Array(s.utf8)
    guard b.count == 10, b[4] == 45, b[7] == 45,
      b.enumerated().allSatisfy({
        $0.offset == 4 || $0.offset == 7 || (48...57).contains($0.element)
      })
    else { return false }
    guard let y = Int(s.prefix(4)), let m = Int(s.dropFirst(5).prefix(2)), let d = Int(s.suffix(2)),
      (1...9999).contains(y), (1...12).contains(m)
    else { return false }
    let leap = y % 400 == 0 || (y % 4 == 0 && y % 100 != 0)
    let days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    return (1...days[m - 1]).contains(d)
  }
  static func url(_ s: String) -> Bool {
    guard !s.isEmpty,
      !s.unicodeScalars.contains(where: {
        CharacterSet.whitespacesAndNewlines.contains($0)
          || CharacterSet.controlCharacters.contains($0)
      }), let u = URLComponents(string: s), let scheme = u.scheme?.lowercased(),
      ["http", "https"].contains(scheme), u.host?.isEmpty == false, u.user == nil, u.password == nil
    else { return false }
    return true
  }
  struct TextBounds: Codable {
    let maxUtf8Bytes: Int
    let maxUnicodeScalars: Int?
    var valid: Bool {
      maxUtf8Bytes == 65_536 && (maxUnicodeScalars.map({ $0 >= 0 && $0 <= 65_536 }) ?? true)
    }
  }
  struct OptionDraft: Codable, Identifiable {
    let id = UUID()
    var label: String
    var tone: Tone = .neutral
    enum CodingKeys: String, CodingKey { case label, tone }
    var valid: Bool { CrewdManagedTableV3Wire.label(label, limit: 120) }
  }
  struct OptionDefinition: Codable, Identifiable {
    let id: String
    let label: String
    let tone: Tone
    let active: Bool
    var valid: Bool { optionNumber(id) != nil && CrewdManagedTableV3Wire.label(label, limit: 120) }
  }
  struct OptionRef: Codable {
    let tableId: String
    let fieldId: String
    let optionId: String
    var valid: Bool {
      objectID(tableId, "T") && V2.fieldNumber(fieldId) != nil && optionNumber(optionId) != nil
    }
  }
  static func optionNumber(_ s: String) -> UInt64? {
    guard s.first == "e", let n = number(String(s.dropFirst())), n > 0 else { return nil }
    return n
  }
  struct KindDraft: Codable {
    var kind: TypeName
    var unit: String? = nil
    var displayScale: Int? = nil
    var options: [OptionDraft]? = nil
    var allowHuman: Bool? = nil
    var target: Constraint? = nil
    var many: Bool? = nil
    var valid: Bool {
      if kind == .decimal {
        return (unit.map({ label($0, limit: 32) }) ?? true)
          && (displayScale.map({ (0...18).contains($0) }) ?? true) && options == nil
          && allowHuman == nil && target == nil && many == nil
      }
      if kind == .singleSelect {
        return unit == nil && displayScale == nil
          && options.map({
            (0...12).contains($0.count) && $0.allSatisfy(\.valid)
              && V2.unique($0.map({ asciiLower($0.label) }))
          }) == true && allowHuman == nil && target == nil && many == nil
      }
      if kind == .person {
        return allowHuman != nil && unit == nil && displayScale == nil && options == nil
          && target == nil && many == nil
      }
      if kind == .link {
        return target?.valid == true && many != nil && unit == nil && displayScale == nil
          && options == nil && allowHuman == nil
      }
      return unit == nil && displayScale == nil && options == nil && allowHuman == nil
        && target == nil && many == nil
    }
    var intent: Intent {
      var e: [(String, Intent)] = [("kind", .string(kind.rawValue))]
      switch kind {
      case .decimal:
        e += [
          ("unit", unit.map(Intent.string) ?? .null),
          ("displayScale", displayScale.map({ .unsigned(UInt64($0)) }) ?? .null),
        ]
      case .singleSelect:
        e.append(
          (
            "options",
            .array(
              (options ?? []).map({
                .object([("label", .string($0.label)), ("tone", .string($0.tone.rawValue))])
              }))
          ))
      case .person: e.append(("allowHuman", .bool(allowHuman ?? false)))
      case .link: e += [("target", target!.intent), ("many", .bool(many!))]
      default: break
      }
      return .object(e)
    }
  }
  struct FieldKind: Codable {
    let kind: TypeName
    var bounds: TextBounds? = nil
    var precision: Int? = nil
    var maxScale: Int? = nil
    var unit: String? = nil
    var displayScale: Int? = nil
    var options: [OptionDefinition]? = nil
    var allowHuman: Bool? = nil
    var target: Constraint? = nil
    var many: Bool? = nil
    var isLink: Bool { kind.isLink }
    var valid: Bool {
      if kind == .text || kind == .longText {
        return bounds?.valid == true && precision == nil && maxScale == nil && unit == nil
          && displayScale == nil && options == nil && allowHuman == nil && target == nil
          && many == nil
      }
      if kind == .decimal {
        return bounds == nil && precision == 38 && maxScale == 18
          && (unit.map({ label($0, limit: 32) }) ?? true)
          && (displayScale.map({ (0...18).contains($0) }) ?? true) && options == nil
          && allowHuman == nil && target == nil && many == nil
      }
      if kind == .singleSelect {
        return bounds == nil && precision == nil && maxScale == nil && unit == nil
          && displayScale == nil
          && options.map({
            $0.count <= 256 && $0.filter(\.active).count <= 12 && $0.allSatisfy(\.valid)
              && V2.unique($0.map(\.id)) && V2.unique($0.map({ asciiLower($0.label) }))
          }) == true && allowHuman == nil && target == nil && many == nil
      }
      if kind == .person {
        return allowHuman != nil && bounds == nil && precision == nil && maxScale == nil
          && unit == nil && displayScale == nil && options == nil && target == nil && many == nil
      }
      if kind == .link {
        return target?.valid == true && many != nil && bounds == nil && precision == nil
          && maxScale == nil && unit == nil && displayScale == nil && options == nil
          && allowHuman == nil
      }
      return bounds == nil && precision == nil && maxScale == nil && unit == nil
        && displayScale == nil && options == nil && allowHuman == nil && target == nil
        && many == nil
    }
    var title: String {
      if kind == .text || kind == .longText {
        return kind.title
          + (bounds?.maxUnicodeScalars.map({ " · " + String($0) + " 字符" }) ?? " · 原文本范围")
      }
      return kind.title
    }
    var constraint: Constraint? {
      kind == .person ? .init(kind: allowHuman == true ? "anyReadable" : "session") : target
    }
    var multiple: Bool { kind == .link && many == true }
  }
  struct FieldDraft: Codable {
    var name: String
    var kind: KindDraft
    var nullable: Bool
    var valid: Bool { label(name, limit: 120) && kind.valid }
    var intent: Intent {
      .object([("name", .string(name)), ("kind", kind.intent), ("nullable", .bool(nullable))])
    }
  }
  struct Field: Codable, Identifiable {
    let id: String
    let name: String
    let kind: FieldKind
    let nullable: Bool
    let hidden: Bool
    var valid: Bool { V2.fieldNumber(id) != nil && label(name, limit: 120) && kind.valid }
  }
  static func asciiLower(_ s: String) -> String {
    String(decoding: s.utf8.map({ (65...90).contains($0) ? $0 + 32 : $0 }), as: UTF8.self)
  }
  enum Cell: Codable, Equatable {
    case null, text(String), integer(String), real(Double), decimal(String), singleSelect(
      OptionRef), date(String), boolean(Bool), url(String)
    enum Keys: String, CodingKey { case type, value }
    init(from decoder: Decoder) throws {
      let c = try decoder.container(keyedBy: Keys.self)
      let type = try c.decode(String.self, forKey: .type)
      switch type {
      case "null":
        guard !c.contains(.value) else { throw Failure.invalid }
        self = .null
      case "text": self = .text(try c.decode(String.self, forKey: .value))
      case "integer": self = .integer(try c.decode(String.self, forKey: .value))
      case "real":
        let v = try c.decode(Double.self, forKey: .value)
        guard v.isFinite else { throw Failure.invalid }
        self = .real(v)
      case "decimal": self = .decimal(try c.decode(String.self, forKey: .value))
      case "singleSelect": self = .singleSelect(try c.decode(OptionRef.self, forKey: .value))
      case "date": self = .date(try c.decode(String.self, forKey: .value))
      case "boolean": self = .boolean(try c.decode(Bool.self, forKey: .value))
      case "url": self = .url(try c.decode(String.self, forKey: .value))
      default: throw Failure.invalid
      }
    }
    func encode(to encoder: Encoder) throws {
      var c = encoder.container(keyedBy: Keys.self)
      switch self {
      case .null: try c.encode("null", forKey: .type)
      case .text(let s):
        try c.encode("text", forKey: .type)
        try c.encode(s, forKey: .value)
      case .integer(let s):
        try c.encode("integer", forKey: .type)
        try c.encode(s, forKey: .value)
      case .real(let d):
        guard d.isFinite else { throw Failure.invalid }
        try c.encode("real", forKey: .type)
        try c.encode(d, forKey: .value)
      case .decimal(let s):
        try c.encode("decimal", forKey: .type)
        try c.encode(s, forKey: .value)
      case .singleSelect(let v):
        try c.encode("singleSelect", forKey: .type)
        try c.encode(v, forKey: .value)
      case .date(let s):
        try c.encode("date", forKey: .type)
        try c.encode(s, forKey: .value)
      case .boolean(let b):
        try c.encode("boolean", forKey: .type)
        try c.encode(b, forKey: .value)
      case .url(let s):
        try c.encode("url", forKey: .type)
        try c.encode(s, forKey: .value)
      }
    }
    static func == (a: Self, b: Self) -> Bool {
      switch (a, b) {
      case (.null, .null): true
      case (.text(let x), .text(let y)), (.integer(let x), .integer(let y)),
        (.decimal(let x), .decimal(let y)), (.date(let x), .date(let y)),
        (.url(let x), .url(let y)):
        bytes(x, y)
      case (.real(let x), .real(let y)): x.bitPattern == y.bitPattern
      case (.boolean(let x), .boolean(let y)): x == y
      case (.singleSelect(let x), .singleSelect(let y)): exact(x, y)
      default: false
      }
    }
    var intent: Intent {
      switch self {
      case .null: .object([("type", .string("null"))])
      case .text(let s): scalarIntent("text", .string(s))
      case .integer(let s): scalarIntent("integer", .string(s))
      case .real(let d): scalarIntent("real", .real(d))
      case .decimal(let s): scalarIntent("decimal", .string(s))
      case .date(let s): scalarIntent("date", .string(s))
      case .boolean(let b): scalarIntent("boolean", .bool(b))
      case .url(let s): scalarIntent("url", .string(s))
      case .singleSelect(let r):
        scalarIntent(
          "singleSelect",
          .object([
            ("tableId", .string(r.tableId)), ("fieldId", .string(r.fieldId)),
            ("optionId", .string(r.optionId)),
          ]))
      }
    }
    var valid: Bool {
      switch self {
      case .null, .boolean: return true
      case .text(let s): return s.utf8.count <= 65_536
      case .integer(let s): return Int64(s).map({ String($0) == s }) == true
      case .real(let d): return d.isFinite
      case .decimal(let s): return CrewdManagedTableV3Wire.decimal(s)
      case .date(let s): return CrewdManagedTableV3Wire.date(s)
      case .url(let s): return s.utf8.count <= 65_536 && CrewdManagedTableV3Wire.url(s)
      case .singleSelect(let r): return r.valid
      }
    }
    var display: String {
      switch self {
      case .null: "空值"
      case .text(let s), .integer(let s), .decimal(let s), .date(let s), .url(let s): s
      case .real(let d): String(d)
      case .boolean(let b): b ? "是" : "否"
      case .singleSelect(let r): r.optionId
      }
    }
    func display(_ field: Field) -> String {
      if case .singleSelect(let r) = self,
        let option = field.kind.options?.first(where: { $0.id == r.optionId })
      {
        return option.label + (option.active ? "" : "（已停用）")
      }
      if case .decimal(let raw) = self {
        var formatted = raw
        if let scale = field.kind.displayScale {
          let fraction =
            raw.split(separator: ".", omittingEmptySubsequences: false).dropFirst().first?.count
            ?? 0
          if fraction < scale {
            formatted +=
              (fraction == 0 ? "." : "") + String(repeating: "0", count: scale - fraction)
          }
        }
        return formatted + (field.kind.unit.map({ " " + $0 }) ?? "")
      }
      return display
    }
    func accepts(_ field: Field, tableId: String, allowInactive: Bool) -> Bool {
      if self == .null { return field.nullable && !field.kind.isLink }
      switch (self, field.kind.kind) {
      case (.text(let s), .text), (.text(let s), .longText):
        return field.kind.bounds.map({ CrewdManagedTableV3Wire.text(s, bounds: $0) }) == true
      case (.integer(let s), .integer): return Int64(s).map({ String($0) == s }) == true
      case (.real(let d), .real): return d.isFinite
      case (.decimal(let s), .decimal): return CrewdManagedTableV3Wire.decimal(s)
      case (.date(let s), .date): return CrewdManagedTableV3Wire.date(s)
      case (.boolean, .boolean): return true
      case (.url(let s), .url): return s.utf8.count <= 65_536 && CrewdManagedTableV3Wire.url(s)
      case (.singleSelect(let r), .singleSelect):
        return r.valid && bytes(r.tableId, tableId) && r.fieldId == field.id
          && field.kind.options?.contains(where: {
            $0.id == r.optionId && (allowInactive || $0.active)
          }) == true
      default: return false
      }
    }
  }
  private static func scalarIntent(_ type: String, _ value: Intent) -> Intent {
    .object([("type", .string(type)), ("value", value)])
  }
  struct ScalarValue: Codable {
    let fieldId: String
    let value: Cell
  }
  struct Actor: Codable {
    let kind: String
    let sessionId: String?
    var valid: Bool {
      kind == "human"
        ? sessionId == nil : kind == "session" && sessionId.map({ V2.opaque($0) }) == true
    }
  }
  struct Lifecycle: Codable {
    let archivedAt: String?
    let archivedBy: Actor?
    let reason: String?
    var archived: Bool { archivedAt != nil }
    var valid: Bool {
      if let archivedAt {
        return CrewdManagedDirectoryWire.managedTime(archivedAt) != nil && archivedBy?.valid == true
          && reason.map({ label($0, limit: 1500) }) == true
      }
      return archivedBy == nil && reason == nil
    }
  }
  struct FrozenReason: Codable {
    let kind: String
    var valid: Bool { ["crewArchived", "tableArchived", "rowArchived"].contains(kind) }
  }
  struct Table: Codable, Identifiable {
    let id: String, crewId: String, schemaRevision: String, title: String
    let purpose: String?
    let titleFieldId: String
    let rowWritePolicy: Policy
    let author: Author
    let fields: [Field]
    let nextFieldNumber: String, nextOptionNumber: String
    let lifecycle: Lifecycle
    let frozenReasons: [FrozenReason]
    let v1Readable: Bool, v1Writable: Bool, v2Readable: Bool, v2Writable: Bool
    var frozen: Bool { !frozenReasons.isEmpty }
    var active: [Field] { fields.filter { !$0.hidden } }
    var valid: Bool {
      guard objectID(id, "T"), V2.opaque(crewId), positive(schemaRevision),
        V2.opaque(title, max: 4096), author.valid, (1...64).contains(fields.count),
        fields.allSatisfy(\.valid), V2.unique(fields.map(\.id)), let next = number(nextFieldNumber),
        next > 0, fields.allSatisfy({ V2.fieldNumber($0.id).map({ $0 < next }) == true }),
        let optionNext = number(nextOptionNumber), optionNext > 0, lifecycle.valid,
        frozenReasons.allSatisfy(\.valid), V2.unique(frozenReasons.map(\.kind)),
        !frozenReasons.contains(where: { $0.kind == "rowArchived" }),
        lifecycle.archived == frozenReasons.contains(where: { $0.kind == "tableArchived" }),
        active.contains(where: { $0.id == titleFieldId && !$0.kind.isLink })
      else { return false }
      let optionIDs = fields.flatMap({ $0.kind.options?.map(\.id) ?? [] })
      return V2.unique(optionIDs)
        && optionIDs.allSatisfy { optionNumber($0).map({ $0 < optionNext }) == true }
    }
    func field(_ id: String) -> Field? { fields.first { $0.id == id } }
  }
  struct Row: Codable, Identifiable {
    let id: String, crewId: String, tableId: String, schemaRevision: String, revision: String
    let author: Author
    let scalarCells: [ScalarValue]
    let linkFieldIds: [String]
    let lifecycle: Lifecycle
    let frozenReasons: [FrozenReason]
    var frozen: Bool { !frozenReasons.isEmpty }
    func scalar(_ field: String) -> Cell {
      scalarCells.first { $0.fieldId == field }?.value ?? .null
    }
    func matches(_ table: Table) -> Bool {
      guard table.valid, objectID(id, "O"), bytes(crewId, table.crewId), tableId == table.id,
        schemaRevision == table.schemaRevision, positive(revision), author.valid, lifecycle.valid,
        frozenReasons.allSatisfy(\.valid), V2.unique(frozenReasons.map(\.kind)),
        lifecycle.archived == frozenReasons.contains(where: { $0.kind == "rowArchived" }),
        V2.unique(scalarCells.map(\.fieldId)), V2.unique(linkFieldIds),
        scalarCells.allSatisfy({ c in
          table.field(c.fieldId).map({ c.value.accepts($0, tableId: table.id, allowInactive: true) }
          ) == true
        }), linkFieldIds.allSatisfy({ table.field($0)?.kind.isLink == true })
      else { return false }
      let expected = Set(table.frozenReasons.map(\.kind)).union(
        lifecycle.archived ? ["rowArchived"] : [])
      return Set(frozenReasons.map(\.kind)) == expected
    }
  }
  struct CellPatch: Codable {
    var kind: String
    var value: Cell? = nil
    var values: [LinkWrite]? = nil
    var keepHiddenSetToken: String? = nil
    var valid: Bool {
      switch kind {
      case "keep": return value == nil && values == nil && keepHiddenSetToken == nil
      case "setScalar": return value?.valid == true && values == nil && keepHiddenSetToken == nil
      case "replaceLinks":
        return value == nil && values.map({ $0.count <= 64 && $0.allSatisfy(\.valid) }) == true
          && (keepHiddenSetToken.map({ V2.opaque($0) }) ?? true)
      default: return false
      }
    }
    var intent: Intent {
      if kind == "keep" { return .object([("kind", .string(kind))]) }
      if kind == "setScalar" {
        return .object([("kind", .string(kind)), ("value", value!.intent)])
      }
      return .object([
        ("kind", .string(kind)), ("values", .array(values!.map(\.intent))),
        ("keepHiddenSetToken", keepHiddenSetToken.map(Intent.string) ?? .null),
      ])
    }
  }
  struct Patch: Codable {
    let fieldId: String
    var change: CellPatch
    var valid: Bool { V2.fieldNumber(fieldId) != nil && change.valid }
    var intent: Intent { .object([("fieldId", .string(fieldId)), ("change", change.intent)]) }
  }
  struct SchemaChange: Codable {
    var kind: String
    var field: FieldDraft? = nil
    var fieldId: String? = nil
    var name: String? = nil
    var title: String? = nil
    var purpose: String? = nil
    var definitionCheckToken: String? = nil
    var confirmNew: Bool? = nil
    var option: OptionRef? = nil
    var optionDraft: OptionDraft? = nil
    var optionLabel: String? = nil
    var tone: Tone? = nil
    // addOption uses wire key `option` (draft), renameOption uses `label`.
    enum CodingKeys: String, CodingKey {
      case kind, field, fieldId, name, title, purpose, definitionCheckToken, confirmNew, option,
        label, tone
    }
    init(
      kind: String, field: FieldDraft? = nil, fieldId: String? = nil, name: String? = nil,
      title: String? = nil, purpose: String? = nil, definitionCheckToken: String? = nil,
      confirmNew: Bool? = nil, option: OptionRef? = nil, optionDraft: OptionDraft? = nil,
      optionLabel: String? = nil, tone: Tone? = nil
    ) {
      self.kind = kind
      self.field = field
      self.fieldId = fieldId
      self.name = name
      self.title = title
      self.purpose = purpose
      self.definitionCheckToken = definitionCheckToken
      self.confirmNew = confirmNew
      self.option = option
      self.optionDraft = optionDraft
      self.optionLabel = optionLabel
      self.tone = tone
    }
    init(from decoder: Decoder) throws {
      let c = try decoder.container(keyedBy: CodingKeys.self)
      kind = try c.decode(String.self, forKey: .kind)
      field = try c.decodeIfPresent(FieldDraft.self, forKey: .field)
      fieldId = try c.decodeIfPresent(String.self, forKey: .fieldId)
      name = try c.decodeIfPresent(String.self, forKey: .name)
      title = try c.decodeIfPresent(String.self, forKey: .title)
      purpose = try c.decodeIfPresent(String.self, forKey: .purpose)
      definitionCheckToken = try c.decodeIfPresent(String.self, forKey: .definitionCheckToken)
      confirmNew = try c.decodeIfPresent(Bool.self, forKey: .confirmNew)
      if kind == "addOption" {
        optionDraft = try c.decode(OptionDraft.self, forKey: .option)
      } else {
        option = try c.decodeIfPresent(OptionRef.self, forKey: .option)
      }
      optionLabel = try c.decodeIfPresent(String.self, forKey: .label)
      tone = try c.decodeIfPresent(Tone.self, forKey: .tone)
    }
    func encode(to encoder: Encoder) throws {
      var c = encoder.container(keyedBy: CodingKeys.self)
      try c.encode(kind, forKey: .kind)
      try c.encodeIfPresent(field, forKey: .field)
      try c.encodeIfPresent(fieldId, forKey: .fieldId)
      try c.encodeIfPresent(name, forKey: .name)
      try c.encodeIfPresent(title, forKey: .title)
      try c.encodeIfPresent(purpose, forKey: .purpose)
      try c.encodeIfPresent(definitionCheckToken, forKey: .definitionCheckToken)
      try c.encodeIfPresent(confirmNew, forKey: .confirmNew)
      if let optionDraft {
        try c.encode(optionDraft, forKey: .option)
      } else {
        try c.encodeIfPresent(option, forKey: .option)
      }
      try c.encodeIfPresent(optionLabel, forKey: .label)
      try c.encodeIfPresent(tone, forKey: .tone)
    }
    var valid: Bool {
      guard allowedSchemaKeys(kind).map({ encodedKeys(self).isSubset(of: $0) }) == true,
        fieldId.map({ V2.fieldNumber($0) != nil }) ?? true
      else { return false }
      switch kind {
      case "addField": return field?.valid == true && fieldId == nil
      case "renameField": return fieldId != nil && name.map({ label($0, limit: 120) }) == true
      case "renameTable":
        return fieldId == nil && title.map({ label($0, limit: 256) }) == true
          && definitionCheckToken.map({ V2.opaque($0, max: 16_384) }) == true && confirmNew != nil
      case "setPurpose":
        return fieldId == nil && purpose.map({ label($0, limit: 1500) }) == true
          && definitionCheckToken.map({ V2.opaque($0, max: 16_384) }) == true && confirmNew != nil
      case "addOption": return fieldId != nil && optionDraft?.valid == true
      case "renameOption":
        return fieldId == option?.fieldId && option?.valid == true
          && optionLabel.map({ label($0, limit: 120) }) == true
      case "deactivateOption", "restoreOption":
        return fieldId == option?.fieldId && option?.valid == true
      case "setOptionTone":
        return fieldId == option?.fieldId && option?.valid == true && tone != nil
      case "hideField", "restoreField", "setTitleField", "widenSingleLink", "widenText",
        "widenNumberToText":
        return fieldId != nil
      default: return false
      }
    }
    var intent: Intent {
      var e: [(String, Intent)] = [("kind", .string(kind))]
      if let field { e.append(("field", field.intent)) }
      if let fieldId { e.append(("fieldId", .string(fieldId))) }
      if let name { e.append(("name", .string(name))) }
      if let title { e.append(("title", .string(title))) }
      if let purpose { e.append(("purpose", .string(purpose))) }
      if let definitionCheckToken {
        e.append(("definitionCheckToken", .string(definitionCheckToken)))
      }
      if let confirmNew { e.append(("confirmNew", .bool(confirmNew))) }
      if let option {
        e.append(
          (
            "option",
            .object([
              ("tableId", .string(option.tableId)), ("fieldId", .string(option.fieldId)),
              ("optionId", .string(option.optionId)),
            ])
          ))
      }
      if let optionDraft {
        e.append(
          (
            "option",
            .object([
              ("label", .string(optionDraft.label)), ("tone", .string(optionDraft.tone.rawValue)),
            ])
          ))
      }
      if let optionLabel { e.append(("label", .string(optionLabel))) }
      if let tone { e.append(("tone", .string(tone.rawValue))) }
      return .object(e)
    }
  }
  struct Operation: Codable {
    var kind: String
    var title: String? = nil
    var purpose: String? = nil
    var fields: [FieldDraft]? = nil
    var titleFieldIndex: Int? = nil
    var rowWritePolicy: NewPolicy? = nil
    var definitionCheckToken: String? = nil
    var confirmNew: Bool? = nil
    var tableId: String? = nil
    var rowId: String? = nil
    var expectedSchemaRevision: String? = nil
    var expectedRowRevision: String? = nil
    var reason: String? = nil
    var change: SchemaChange? = nil
    var patches: [Patch]? = nil
    var target: String { rowId ?? tableId ?? "createTable" }
    var valid: Bool {
      guard allowedOperationKeys(kind).map({ encodedKeys(self).isSubset(of: $0) }) == true else {
        return false
      }
      if let option = change?.option { guard option.tableId == tableId else { return false } }
      if kind == "createTable" {
        guard title.map({ label($0, limit: 256) }) == true,
          purpose.map({ label($0, limit: 1500) }) == true, let fields,
          (1...64).contains(fields.count), fields.allSatisfy(\.valid), let titleFieldIndex,
          fields.indices.contains(titleFieldIndex), !fields[titleFieldIndex].kind.kind.isLink,
          rowWritePolicy != nil, definitionCheckToken.map({ V2.opaque($0, max: 16_384) }) == true,
          confirmNew != nil
        else { return false }
        return true
      }
      guard expectedSchemaRevision.map(positive) == true else { return false }
      if ["alterTable", "addRow", "archiveTable", "restoreTable"].contains(kind) {
        guard tableId.map({ objectID($0, "T") }) == true, rowId == nil else { return false }
      } else if ["patchRow", "archiveRow", "restoreRow"].contains(kind) {
        guard rowId.map({ objectID($0, "O") }) == true, expectedRowRevision.map(positive) == true,
          tableId == nil
        else { return false }
      } else {
        return false
      }
      if kind != "addRow" {
        guard reason.map({ label($0, limit: 1500) }) == true else { return false }
      }
      if kind == "alterTable" { return change?.valid == true }
      if kind == "addRow" || kind == "patchRow" {
        return patches.map({
          $0.count <= 64 && $0.allSatisfy(\.valid) && V2.unique($0.map(\.fieldId))
        }) == true
      }
      return true
    }
    var intent: Intent {
      var e: [(String, Intent)] = [("kind", .string(kind))]
      if let title { e.append(("title", .string(title))) }
      if let purpose { e.append(("purpose", .string(purpose))) }
      if let fields { e.append(("fields", .array(fields.map(\.intent)))) }
      if let titleFieldIndex { e.append(("titleFieldIndex", .unsigned(UInt64(titleFieldIndex)))) }
      if let rowWritePolicy { e.append(("rowWritePolicy", .string(rowWritePolicy.rawValue))) }
      if let definitionCheckToken {
        e.append(("definitionCheckToken", .string(definitionCheckToken)))
      }
      if let confirmNew { e.append(("confirmNew", .bool(confirmNew))) }
      if let tableId { e.append(("tableId", .string(tableId))) }
      if let rowId { e.append(("rowId", .string(rowId))) }
      if let expectedSchemaRevision {
        e.append(("expectedSchemaRevision", .string(expectedSchemaRevision)))
      }
      if let expectedRowRevision { e.append(("expectedRowRevision", .string(expectedRowRevision))) }
      if let reason { e.append(("reason", .string(reason))) }
      if let change { e.append(("change", change.intent)) }
      if let patches { e.append(("patches", .array(patches.map(\.intent)))) }
      return .object(e)
    }
  }
  struct Submit: Codable {
    let scope: Scope
    let crewId: String
    let requestId: String
    let operation: Operation
    var intent: Intent {
      .object([
        ("method", .string(submit)), ("scope", V2.scopeIntent(scope)), ("crewId", .string(crewId)),
        ("requestId", .string(requestId)), ("operation", operation.intent),
      ])
    }
    func digest() throws -> String {
      guard operation.valid else { throw Failure.invalid }
      return try digestIntent(intent)
    }
  }
  struct IssuedOption: Codable {
    let fieldId: String
    let optionId: String
  }
  struct Effect: Codable {
    let kind: String
    let tableId: String
    var rowId: String? = nil
    var schemaRevision: String? = nil
    var fieldIds: [String]? = nil
    var titleFieldId: String? = nil
    var issuedOptions: [IssuedOption]? = nil
    var originalSchemaRevision: String? = nil
    var resultSchemaRevision: String? = nil
    var changedFieldIds: [String]? = nil
    var rowRevision: String? = nil
    var edgeIds: [String]? = nil
    var originalRowRevision: String? = nil
    var resultRowRevision: String? = nil
    var addedEdgeIds: [String]? = nil
    var removedEdgeIds: [String]? = nil
    var changed: Bool? = nil
    var archived: Bool? = nil
  }
  struct Receipt: Codable {
    let scope: Scope
    let crewId: String
    let requestId: String
    let recordedAt: String
    let operationKind: String
    let intentDigest: String
    let eventId: String?
    let effect: Effect
  }
  struct Lookup: Decodable {
    let status: String
    let receipt: Receipt?
  }
  struct TablePage: Decodable {
    let items: [Table]
    let nextAfter: String?
  }
  struct RowPage: Decodable {
    let table: Table
    let items: [Row]
    let nextAfter: String?
  }
  struct RowLinkCell: Decodable {
    let scope: Scope
    let rowId: String
    let schemaRevision: String
    let rowRevision: String
    let cell: LinkCell
  }
  struct NearCandidate: Decodable, Identifiable {
    let tableId: String
    let schemaRevision: String
    let title: String
    let purpose: String?
    let archived: Bool
    var id: String { tableId }
  }
  struct DefinitionCheck: Decodable {
    let scope: Scope
    let crewId: String
    let state: String
    let exactNameTaken: Bool
    let nearCandidates: [NearCandidate]
    let moreSimilar: Bool
    let definitionCheckToken: String?
    let nextCursor: String?
  }
  struct HistorySource: Codable {
    let kind: String
    let id: String
    var valid: Bool { kind == "table" ? objectID(id, "T") : kind == "row" && objectID(id, "O") }
  }
  struct HistoryKey: Codable {
    let kind: String
    var eventId: String? = nil
    var codec: String? = nil
    var requestId: String? = nil
    var entryLocator: String? = nil
    var valid: Bool {
      if kind == "event" {
        return eventId.map(V2.uuid) == true && codec == nil && requestId == nil
          && entryLocator == nil
      }
      return kind == "legacyReceipt" && eventId == nil
        && ["managedObjectsV1", "managedTablesV2"].contains(codec ?? "")
        && requestId.map(V2.uuid) == true
        && entryLocator.map({ V2.opaque($0, max: 16_384) }) == true
    }
    var id: String {
      (try? CrewdOrganizationSearchWire.encoder.encode(self).base64EncodedString()) ?? "invalid"
    }
  }
  struct HistoryPath: Codable {
    let kind: String
    var fieldId: String? = nil
    var side: String? = nil
    var valid: Bool {
      switch kind {
      case "reason": return fieldId == nil && side == nil
      case "schema": return fieldId == nil && ["before", "after"].contains(side ?? "")
      case "scalar", "links":
        return fieldId.flatMap(V2.fieldNumber) != nil && ["before", "after"].contains(side ?? "")
      default: return false
      }
    }
  }
  struct HistorySummary: Decodable, Identifiable {
    let key: HistoryKey
    let action: String
    let actor: Actor
    let recordedAt: String
    let reasonPreview: String?
    let reasonAvailable: Bool
    let changedFieldIds: [String]
    let originalRevision: String?
    let resultRevision: String?
    let legacyDetailsUnavailable: Bool
    let restrictedDetails: Bool
    var id: String { key.id }
  }
  struct HistoryPage: Decodable {
    let scope: Scope
    let source: HistorySource
    let items: [HistorySummary]
    let nextCursor: String?
  }
  struct ValueChunk: Decodable {
    let status: String
    var scope: Scope? = nil
    var source: HistorySource? = nil
    var key: HistoryKey? = nil
    var path: HistoryPath? = nil
    var valueType: String? = nil
    var utf8: String? = nil
    var nextCursor: String? = nil
    var visible: [V2.VisibleLink]? = nil
    var restrictedDetails: Bool? = nil
  }

  static func intentBytes(_ intent: Intent) throws -> Data {
    var raw = try CrewdManagedV2Intent.bytes(intent)
    let prefix = Data("managed-v2-intent.v1\0".utf8)
    guard raw.starts(with: prefix) else { throw Failure.invalid }
    raw.replaceSubrange(0..<prefix.count, with: Data("managed-v3-intent.v1\0".utf8))
    return raw
  }
  static func digestIntent(_ intent: Intent) throws -> String {
    SHA256.hash(data: try intentBytes(intent)).map({ String(format: "%02x", $0) }).joined()
  }
}

extension CrewdManagedTableV3Wire {
  static func encodedKeys<T: Encodable>(_ value: T) -> Set<String> {
    guard let raw = try? JSONDecoder().decode(CrewdValue.self, from: JSONEncoder().encode(value)),
      case .object(let values) = raw
    else { return ["invalid"] }
    return Set(values.keys)
  }
  static func allowedOperationKeys(_ kind: String) -> Set<String>? {
    switch kind {
    case "createTable":
      return [
        "kind", "title", "purpose", "fields", "titleFieldIndex", "rowWritePolicy",
        "definitionCheckToken", "confirmNew",
      ]
    case "alterTable": return ["kind", "tableId", "expectedSchemaRevision", "reason", "change"]
    case "addRow": return ["kind", "tableId", "expectedSchemaRevision", "patches"]
    case "patchRow":
      return [
        "kind", "rowId", "expectedSchemaRevision", "expectedRowRevision", "reason", "patches",
      ]
    case "archiveTable", "restoreTable":
      return ["kind", "tableId", "expectedSchemaRevision", "reason"]
    case "archiveRow", "restoreRow":
      return ["kind", "rowId", "expectedSchemaRevision", "expectedRowRevision", "reason"]
    default: return nil
    }
  }
  static func allowedSchemaKeys(_ kind: String) -> Set<String>? {
    switch kind {
    case "addField": return ["kind", "field"]
    case "renameField": return ["kind", "fieldId", "name"]
    case "renameTable": return ["kind", "title", "definitionCheckToken", "confirmNew"]
    case "setPurpose": return ["kind", "purpose", "definitionCheckToken", "confirmNew"]
    case "addOption": return ["kind", "fieldId", "option"]
    case "renameOption": return ["kind", "fieldId", "option", "label"]
    case "deactivateOption", "restoreOption": return ["kind", "fieldId", "option"]
    case "setOptionTone": return ["kind", "fieldId", "option", "tone"]
    case "hideField", "restoreField", "setTitleField", "widenSingleLink", "widenText",
      "widenNumberToText":
      return ["kind", "fieldId"]
    default: return nil
    }
  }
  static func fields(_ raw: CrewdValue, _ allowed: Set<String>) throws {
    guard case .object(let f) = raw, Set(f.keys).isSubset(of: allowed) else {
      throw Failure.invalid
    }
  }
  static func closedCell(_ raw: CrewdValue) throws {
    guard let type = raw["type"].string else { throw Failure.invalid }
    try fields(raw, type == "null" ? ["type"] : ["type", "value"])
    if type == "singleSelect" { try fields(raw["value"], ["tableId", "fieldId", "optionId"]) }
  }
  static func closedKind(_ raw: CrewdValue, draft: Bool = false) throws {
    guard let tag = raw["kind"].string, let type = TypeName(rawValue: tag) else {
      throw Failure.invalid
    }
    var keys: Set<String> = ["kind"]
    switch type {
    case .text, .longText:
      if !draft {
        keys.insert("bounds")
        try fields(raw["bounds"], ["maxUtf8Bytes", "maxUnicodeScalars"])
      }
    case .decimal:
      keys.formUnion(
        draft ? ["unit", "displayScale"] : ["precision", "maxScale", "unit", "displayScale"])
    case .singleSelect:
      keys.insert("options")
      guard case .array(let values) = raw["options"] else { throw Failure.invalid }
      for v in values {
        try fields(v, draft ? ["label", "tone"] : ["id", "label", "tone", "active"])
      }
    case .person: keys.insert("allowHuman")
    case .link:
      keys.formUnion(["target", "many"])
      try fields(raw["target"], ["kind", "tableId"])
    default: break
    }
    try fields(raw, keys)
  }
  static func closedField(_ raw: CrewdValue, draft: Bool = false) throws {
    try fields(
      raw, draft ? ["name", "kind", "nullable"] : ["id", "name", "kind", "nullable", "hidden"])
    try closedKind(raw["kind"], draft: draft)
  }
  static func closedLifecycle(_ raw: CrewdValue) throws {
    try fields(raw, ["archivedAt", "archivedBy", "reason"])
    if raw["archivedBy"] != .null { try fields(raw["archivedBy"], ["kind", "sessionId"]) }
  }
  static func closedTable(_ raw: CrewdValue) throws {
    try fields(
      raw,
      [
        "id", "crewId", "schemaRevision", "title", "purpose", "titleFieldId", "rowWritePolicy",
        "author", "fields", "nextFieldNumber", "nextOptionNumber", "lifecycle", "frozenReasons",
        "v1Readable", "v1Writable", "v2Readable", "v2Writable",
      ])
    try fields(raw["author"], ["kind", "sessionId"])
    try closedLifecycle(raw["lifecycle"])
    guard case .array(let fs) = raw["fields"], case .array(let frozen) = raw["frozenReasons"] else {
      throw Failure.invalid
    }
    for f in fs { try closedField(f) }
    for f in frozen { try fields(f, ["kind"]) }
  }
  static func closedRow(_ raw: CrewdValue) throws {
    try fields(
      raw,
      [
        "id", "crewId", "tableId", "schemaRevision", "revision", "author", "scalarCells",
        "linkFieldIds", "lifecycle", "frozenReasons",
      ])
    try fields(raw["author"], ["kind", "sessionId"])
    try closedLifecycle(raw["lifecycle"])
    guard case .array(let cs) = raw["scalarCells"], case .array(let frozen) = raw["frozenReasons"]
    else { throw Failure.invalid }
    for c in cs {
      try fields(c, ["fieldId", "value"])
      try closedCell(c["value"])
    }
    for f in frozen { try fields(f, ["kind"]) }
  }
  static func closedScope(_ raw: CrewdValue) throws { try fields(raw, ["dbId", "restoreEpoch"]) }
  static func closedReceipt(_ raw: CrewdValue) throws {
    try fields(
      raw,
      [
        "scope", "crewId", "requestId", "recordedAt", "operationKind", "intentDigest", "eventId",
        "effect",
      ])
    try closedScope(raw["scope"])
    let e = raw["effect"]
    var keys: Set<String> = ["kind", "tableId"]
    switch e["kind"].string {
    case "tableCreated":
      keys.formUnion(["schemaRevision", "fieldIds", "titleFieldId", "issuedOptions"])
    case "schemaChanged":
      keys.formUnion([
        "originalSchemaRevision", "resultSchemaRevision", "changedFieldIds", "issuedOptions",
        "changed",
      ])
    case "rowAdded": keys.formUnion(["rowId", "schemaRevision", "rowRevision", "edgeIds"])
    case "rowPatched":
      keys.formUnion([
        "rowId", "schemaRevision", "originalRowRevision", "resultRowRevision", "changedFieldIds",
        "addedEdgeIds", "removedEdgeIds", "changed",
      ])
    case "tableLifecycle":
      keys.formUnion(["originalSchemaRevision", "resultSchemaRevision", "archived", "changed"])
    case "rowLifecycle":
      keys.formUnion([
        "rowId", "schemaRevision", "originalRowRevision", "resultRowRevision", "archived",
        "changed",
      ])
    default: throw Failure.invalid
    }
    try fields(e, keys)
    if case .array(let options) = e["issuedOptions"] {
      for o in options { try fields(o, ["fieldId", "optionId"]) }
    }
  }
  static func closedHistoryKey(_ raw: CrewdValue) throws {
    try fields(
      raw,
      raw["kind"].string == "event"
        ? ["kind", "eventId"] : ["kind", "codec", "requestId", "entryLocator"])
  }
  static func closedHistoryPath(_ raw: CrewdValue) throws {
    switch raw["kind"].string {
    case "reason": try fields(raw, ["kind"])
    case "schema": try fields(raw, ["kind", "side"])
    case "scalar", "links": try fields(raw, ["kind", "fieldId", "side"])
    default: throw Failure.invalid
    }
  }
  static func closedLinks(_ raw: CrewdValue) throws {
    try fields(raw, ["fieldId", "visible", "hiddenSet", "incompleteRequiredLink"])
    if raw["hiddenSet"] != .null { try fields(raw["hiddenSet"], ["keepToken"]) }
    guard case .array(let visible) = raw["visible"] else { throw Failure.invalid }
    for v in visible {
      try fields(v, ["edgeId", "target", "summary"])
      try CrewdOrganizationSearchWire.closedTarget(v["target"])
    }
  }
  static func closed(_ raw: CrewdValue, method: String) throws {
    switch method {
    case tableGet: try closedTable(raw)
    case rowGet: try closedRow(raw)
    case tableList:
      try fields(raw, ["items", "nextAfter"])
      guard case .array(let items) = raw["items"] else { throw Failure.invalid }
      for i in items { try closedTable(i) }
    case rows:
      try fields(raw, ["table", "items", "nextAfter"])
      try closedTable(raw["table"])
      guard case .array(let items) = raw["items"] else { throw Failure.invalid }
      for i in items { try closedRow(i) }
    case rowLinks:
      try fields(raw, ["scope", "rowId", "schemaRevision", "rowRevision", "cell"])
      try closedScope(raw["scope"])
      try closedLinks(raw["cell"])
    case submit: try closedReceipt(raw)
    case lookup:
      try fields(raw, ["status", "receipt"])
      if raw["receipt"] != .null { try closedReceipt(raw["receipt"]) }
    case defineCheck:
      try fields(
        raw,
        [
          "scope", "crewId", "state", "exactNameTaken", "nearCandidates", "moreSimilar",
          "definitionCheckToken", "nextCursor",
        ])
      try closedScope(raw["scope"])
      guard case .array(let items) = raw["nearCandidates"] else { throw Failure.invalid }
      for i in items {
        try fields(i, ["tableId", "schemaRevision", "title", "purpose", "archived"])
      }
    case history:
      try fields(raw, ["scope", "source", "items", "nextCursor"])
      try closedScope(raw["scope"])
      try fields(raw["source"], ["kind", "id"])
      guard case .array(let items) = raw["items"] else { throw Failure.invalid }
      for i in items {
        try fields(
          i,
          [
            "key", "action", "actor", "recordedAt", "reasonPreview", "reasonAvailable",
            "changedFieldIds", "originalRevision", "resultRevision", "legacyDetailsUnavailable",
            "restrictedDetails",
          ])
        try closedHistoryKey(i["key"])
        try fields(i["actor"], ["kind", "sessionId"])
      }
    case historyValue:
      if ["restricted", "unavailable"].contains(raw["status"].string ?? "") {
        try fields(raw, ["status"])
        return
      }
      try fields(
        raw,
        raw["status"].string == "scalar"
          ? ["status", "scope", "source", "key", "path", "valueType", "utf8", "nextCursor"]
          : [
            "status", "scope", "source", "key", "path", "visible", "restrictedDetails",
            "nextCursor",
          ])
      try closedScope(raw["scope"])
      try fields(raw["source"], ["kind", "id"])
      try closedHistoryKey(raw["key"])
      try closedHistoryPath(raw["path"])
      if case .array(let visible) = raw["visible"] {
        for v in visible {
          try fields(v, ["edgeId", "target", "summary"])
          try CrewdOrganizationSearchWire.closedTarget(v["target"])
        }
      }
    default: throw Failure.invalid
    }
  }
  /// Core enforces the exact persisted JSON budget. JSONEncoder and serde spell
  /// finite Reals differently (for example 1e+30 / 1e30). A one-byte Real is a
  /// safe lower bound, so this preflight never narrows a valid legacy row merely
  /// because of the client's number formatter. The actual submitted value stays intact.
  static func scalarBudgetLowerBound(_ cells: [String: Cell]) throws -> Int {
    let lower = cells.mapValues { value in
      if case .real = value { return Cell.real(0) }
      return value
    }
    return try byteCount(lower)
  }
  static func byteCount<T: Encodable>(_ value: T) throws -> Int {
    let e = JSONEncoder()
    e.outputFormatting = [.withoutEscapingSlashes]
    return try e.encode(value).count
  }
}

extension CrewdManagedTableV3Wire {
  /// Only changed stable fields become patches. Unloaded relationships always stay Keep.
  static func rowPatches(
    table: Table, row: Row?, edits: [String: CellPatch], loaded: [String: LinkCell]
  ) throws -> [Patch] {
    guard table.valid, row.map({ $0.matches(table) }) ?? true else { throw Failure.invalid }
    var scalar = Dictionary(
      uniqueKeysWithValues: (row?.scalarCells ?? []).map({ ($0.fieldId, $0.value) }))
    var result: [Patch] = []
    for field in table.active {
      let change = edits[field.id] ?? .init(kind: "keep")
      guard change.valid else { throw Failure.bounds(field.name) }
      if change.kind == "keep" {
        if row == nil && !field.nullable { throw Failure.required(field.name) }
        if row != nil { result.append(.init(fieldId: field.id, change: change)) }
        continue
      }
      if field.kind.isLink {
        guard change.kind == "replaceLinks", let values = change.values, values.count <= 64,
          field.kind.multiple || values.count <= 1
        else { throw Failure.bounds(field.name) }
        let old = loaded[field.id]
        if row != nil {
          guard let old else { throw Failure.unloaded(field.name) }
          guard old.hiddenSet?.keepToken == change.keepHiddenSetToken,
            old.hiddenSet == nil || values.count <= old.visible.count
          else { throw Failure.bounds(field.name) }
        } else if change.keepHiddenSetToken != nil {
          throw Failure.invalid
        }
        if !field.nullable && values.isEmpty && old?.hiddenSet == nil {
          throw Failure.required(field.name)
        }
        if field.kind.kind == .person {
          guard
            values.allSatisfy({
              $0.kind == "reference"
                && ($0.target?.kind == .session
                  || (field.kind.allowHuman == true && $0.target?.kind == .human))
            })
          else { throw Failure.bounds(field.name) }
        }
      } else {
        guard change.kind == "setScalar", let value = change.value,
          value.accepts(field, tableId: table.id, allowInactive: false)
        else { throw Failure.bounds(field.name) }
        scalar[field.id] = value
      }
      result.append(.init(fieldId: field.id, change: change))
    }
    guard edits.keys.allSatisfy({ id in table.active.contains { $0.id == id } }) else {
      throw Failure.invalid
    }
    guard try scalarBudgetLowerBound(scalar) <= 65_536 else {
      throw Failure.bounds("整行文本与数值内容（最多64KB）")
    }
    return result
  }
}
