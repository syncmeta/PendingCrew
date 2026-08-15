// SHIM — replaces PendingBot's `APIClient().download(_:)` at the two call
// sites in BubbleView's AttachmentGrid (saveToAlbum) and FileAttachmentChip
// (openFile). PendingBot's APIClient is Supabase-JWT-backed; PendingCrew uses
// device-grant bearer tokens accessed via AppModel.imageAuth.
//
// Call shape is a one-to-one substitute:
//
//   PendingBot:   try await APIClient().download("v1/uploads/\(att.id)")
//   PendingCrew:  try await CrewAttachmentDownload.data(path: "v1/uploads/\(att.id)", auth: auth)
//
// If `auth` is nil (user not logged in / baseURL invalid) the call throws
// `CrewAttachmentDownloadError.notAuthenticated` so callers can surface
// an error state rather than crashing.

import Foundation

enum CrewAttachmentDownloadError: Error {
    case notAuthenticated
}

enum CrewAttachmentDownload {
    /// Fetch an auth-gated upload by its relative path (e.g. "v1/uploads/<id>").
    /// `auth` is `AppModel.imageAuth` — a `(baseURL: URL, token: String)?`.
    /// Pass it as a non-optional; the caller should guard-unwrap first and
    /// throw / bail out if nil (mirrors how the vendored code handles network
    /// errors: it lets the `do/catch` or `try?` surface the failure).
    static func data(path: String, auth: (baseURL: URL, token: String)) async throws -> Data {
        let url = auth.baseURL.appendingPathComponent(path)
        var req = URLRequest(url: url)
        req.setValue("Bearer \(auth.token)", forHTTPHeaderField: "Authorization")
        let (data, _) = try await URLSession.shared.data(for: req)
        return data
    }
}
