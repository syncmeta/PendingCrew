// SHIM for PendingBot Components/UserAvatar.swift — renders via vendored BotAvatar.
//
// PendingBot's UserAvatar prefers an uploaded photo (`/v1/uploads/<id>`) when
// `attachmentId` is non-nil, falling back to BotAvatar(seed:size:) otherwise.
//
// PendingCrew does not currently support uploaded user profile photos
// (crew members are identified by their agent identity, not photos). So this
// shim always renders BotAvatar keyed off `seed` — the emoji + pastel circle
// that already displays consistently across PendingBot and PendingCrew for
// the same seed value.
//
// Public init signature is IDENTICAL to PendingBot's UserAvatar so that
// BubbleView constructs it unchanged:
//
//   UserAvatar(seed: g.avatarSeed, attachmentId: g.avatarPath, size: 30)
//   UserAvatar(seed: peer.avatarSeed, attachmentId: peer.avatarPath, size: 30)
//   UserAvatar(seed: s.seed, attachmentId: s.attachmentId, size: 30)
//
// When PendingCrew adds uploaded-photo support, replace this shim with a real
// implementation that renders the referenced attachment.

import SwiftUI

struct UserAvatar: View {
    let seed: String
    let attachmentId: String?
    var size: CGFloat = 36

    var body: some View {
        // PendingCrew has no uploaded user avatars yet — always use the
        // deterministic emoji avatar keyed off seed.
        // TODO(tech-debt): once user avatar uploads land, load via CrewRemoteImage
        // when attachmentId is non-nil (mirror PendingBot's UserAvatar body).
        BotAvatar(seed: seed, size: size)
    }
}
