import AppKit
import SwiftUI

struct AboutView: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            header

            Text("Démarre, supervise et orchestre tes process de dev locaux — quel que soit le langage.")
                .font(.system(size: 13))
                .foregroundStyle(.secondary)

            SectionCard(title: "Liens", spacing: 0, padding: EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0)) {
                aboutLink(
                    icon: "building.2",
                    title: "Développé par SkollN",
                    subtitle: "skolln.com",
                    url: "https://www.skolln.com"
                )
                Divider().padding(.leading, 38)
                aboutLink(
                    icon: "sparkles",
                    title: "Découvre aussi Alume",
                    subtitle: "Agrégateur de contenus avec IA intégrée",
                    url: "https://alume.skolln.com"
                )
                Divider().padding(.leading, 38)
                aboutLink(
                    icon: "chevron.left.forwardslash.chevron.right",
                    title: "Code source",
                    subtitle: "Open source sous licence GPLv3",
                    url: "https://github.com/masmarino/switchboard"
                )
            }
        }
        .padding(20)
        .frame(width: 380)
    }

    private var header: some View {
        HStack(spacing: 12) {
            Image(nsImage: NSApplication.shared.applicationIconImage ?? NSImage())
                .resizable()
                .frame(width: 44, height: 44)
                .clipShape(RoundedRectangle(cornerRadius: 11, style: .continuous))
            VStack(alignment: .leading, spacing: 2) {
                Text("Switchboard")
                    .font(.system(size: 18, weight: .bold))
                Text("Version 0.5.0")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            }
            Spacer()
        }
    }

    private func aboutLink(icon: String, title: String, subtitle: String, url: String) -> some View {
        AboutLinkRow(icon: icon, title: title, subtitle: subtitle, url: URL(string: url)!)
    }
}

private struct AboutLinkRow: View {
    let icon: String
    let title: String
    let subtitle: String
    let url: URL

    @State private var isHovering = false

    var body: some View {
        Link(destination: url) {
            HStack(spacing: 10) {
                Image(systemName: icon)
                    .foregroundStyle(Color.switchboardAccent)
                    .frame(width: 20)
                VStack(alignment: .leading, spacing: 1) {
                    Text(title)
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(.primary)
                    Text(subtitle)
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
                Image(systemName: "arrow.up.right")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(
                isHovering ? Color.secondary.opacity(0.12) : .clear,
                in: RoundedRectangle(cornerRadius: 6, style: .continuous)
            )
        }
        .buttonStyle(.plain)
        .onHover { hovering in isHovering = hovering }
    }
}
