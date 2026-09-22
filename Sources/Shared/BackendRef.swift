import Foundation

/// A persisted backend address shared by the macOS viewer and the iOS remote data client.
struct BackendRef: Codable, Equatable, Identifiable {
    enum Transport: Equatable {
        case localSocket(path: String)
        case remote(url: String)
    }

    var id: String
    var displayName: String
    var transport: Transport
    var isBuiltIn: Bool = false

    var isRemote: Bool { if case .remote = transport { return true }; return false }

    private enum CodingKeys: String, CodingKey {
        case id, displayName, kind, address, isBuiltIn
    }

    init(id: String, displayName: String, transport: Transport, isBuiltIn: Bool = false) {
        self.id = id
        self.displayName = displayName
        self.transport = transport
        self.isBuiltIn = isBuiltIn
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        displayName = try c.decode(String.self, forKey: .displayName)
        isBuiltIn = try c.decodeIfPresent(Bool.self, forKey: .isBuiltIn) ?? false
        let address = try c.decode(String.self, forKey: .address)
        switch try c.decode(String.self, forKey: .kind) {
        case "localSocket": transport = .localSocket(path: address)
        case "remote": transport = .remote(url: address)
        case let other:
            throw DecodingError.dataCorruptedError(
                forKey: .kind, in: c, debugDescription: "不认识的后端类型：\(other)")
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(displayName, forKey: .displayName)
        try c.encode(isBuiltIn, forKey: .isBuiltIn)
        switch transport {
        case let .localSocket(path):
            try c.encode("localSocket", forKey: .kind)
            try c.encode(path, forKey: .address)
        case let .remote(url):
            try c.encode("remote", forKey: .kind)
            try c.encode(url, forKey: .address)
        }
    }
}
