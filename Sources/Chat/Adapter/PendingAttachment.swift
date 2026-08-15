// PENDINGCREW SHIM: PendingAttachment adapter
//
// PendingBot defines `PendingAttachment` inside ConversationView.swift (line 2255).
// The vendored ComposerView binds to `[PendingAttachment]` and reads:
//   att.id, att.isImage, att.localPreviewData, att.uploadedAttachmentId,
//   att.filename, att.uploadState (.uploading / .uploaded / .failed)
//
// PendingCrew already has `PendingComposerAttachment` in CrewComposer.swift,
// but its shape differs (no uploadState, no localPreviewData). Rather than
// aliasing, we define a minimal `PendingAttachment` here that exactly matches
// what ComposerView reads. PendingCrew's own composer layer can bridge between
// its `PendingComposerAttachment` and this type at the call site.

import Foundation

/// Composer attachment in flight. Mirrors PendingBot's `PendingAttachment`
/// exactly — ComposerView is vendored verbatim and reads these fields.
struct PendingAttachment: Identifiable, Hashable {
    let id: String
    var remoteId: String? = nil
    let mime: String
    let size: Int
    /// Original filename — set for non-image files, nil for images.
    var filename: String? = nil
    var uploadState: UploadState = .uploaded
    /// Local bytes used for immediate thumbnail preview while the upload is
    /// still in flight. Cleared for server-backed attachments.
    var localPreviewData: Data? = nil
    var errorMessage: String? = nil

    enum UploadState: String, Hashable {
        case uploading
        case uploaded
        case failed
    }

    var isImage: Bool { mime.lowercased().hasPrefix("image/") }
    var isUploaded: Bool { uploadState == .uploaded && uploadedAttachmentId != nil }
    var uploadedAttachmentId: String? { remoteId ?? (uploadState == .uploaded ? id : nil) }
}
