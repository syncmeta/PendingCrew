import SwiftUI

struct PendingCrewPlaceholderIcon: View {
    var size: CGFloat = 56

    var body: some View {
        Image("PendingCrewSymbol")
            .font(.system(size: size, weight: .regular))
            .symbolRenderingMode(.monochrome)
            .foregroundStyle(.tertiary)
            .accessibilityHidden(true)
    }
}
