//
//  TerminalCamCameraApp.swift
//  Thin container app whose only job is activating (and deactivating) the
//  bundled CMIO camera extension. The app must be running from /Applications
//  for activation requests to be accepted by macOS.
//

import SwiftUI
import SystemExtensions

final class ExtensionManager: NSObject, ObservableObject, OSSystemExtensionRequestDelegate {
    static let extensionIdentifier = "com.bluevisor.terminalcam.extension"

    @Published var status = "Ready."

    func install() {
        status = "Requesting extension activation…"
        let request = OSSystemExtensionRequest.activationRequest(
            forExtensionWithIdentifier: Self.extensionIdentifier,
            queue: .main
        )
        request.delegate = self
        OSSystemExtensionManager.shared.submitRequest(request)
    }

    func uninstall() {
        status = "Requesting extension removal…"
        let request = OSSystemExtensionRequest.deactivationRequest(
            forExtensionWithIdentifier: Self.extensionIdentifier,
            queue: .main
        )
        request.delegate = self
        OSSystemExtensionManager.shared.submitRequest(request)
    }

    // MARK: OSSystemExtensionRequestDelegate

    func request(
        _ request: OSSystemExtensionRequest,
        actionForReplacingExtension existing: OSSystemExtensionProperties,
        withExtension ext: OSSystemExtensionProperties
    ) -> OSSystemExtensionRequest.ReplacementAction {
        .replace
    }

    func requestNeedsUserApproval(_ request: OSSystemExtensionRequest) {
        status = """
            Approval needed: open System Settings → General → \
            Login Items & Extensions → Camera Extensions and allow \
            “TerminalCam Camera”, then click Install again.
            """
    }

    func request(
        _ request: OSSystemExtensionRequest,
        didFinishWithResult result: OSSystemExtensionRequest.Result
    ) {
        switch result {
        case .completed:
            status = "Camera extension is installed. “Terminal Cam” is now available in video apps."
        case .willCompleteAfterReboot:
            status = "Extension will finish installing after a reboot."
        @unknown default:
            status = "Finished with unexpected result: \(result)."
        }
    }

    func request(_ request: OSSystemExtensionRequest, didFailWithError error: Error) {
        let nsError = error as NSError
        if nsError.domain == OSSystemExtensionErrorDomain,
            nsError.code == OSSystemExtensionError.unsupportedParentBundleLocation.rawValue
        {
            status = "This app must be moved to /Applications before it can install the extension."
        } else {
            status = "Failed: \(error.localizedDescription)"
        }
    }
}

@main
struct TerminalCamCameraApp: App {
    @StateObject private var manager = ExtensionManager()

    var body: some Scene {
        WindowGroup {
            VStack(alignment: .leading, spacing: 16) {
                Text("TerminalCam Virtual Camera")
                    .font(.title2.bold())
                Text(
                    """
                    Installs the “Terminal Cam” system camera. Feed it by running \
                    `terminal-cam --virtual-cam` in your terminal, then pick \
                    “Terminal Cam” as the camera in Zoom, Meet, FaceTime, etc.
                    """
                )
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

                HStack {
                    Button("Install / Update Extension") { manager.install() }
                        .keyboardShortcut(.defaultAction)
                    Button("Uninstall Extension") { manager.uninstall() }
                }

                Text(manager.status)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(24)
            .frame(width: 460)
            .onAppear { manager.install() }
        }
        .windowResizability(.contentSize)
    }
}
