//
//  CameraExtension.swift
//  CMIO camera extension publishing the "Terminal Cam" virtual camera.
//
//  Device layout follows the standard sink-stream pattern: a source stream
//  that consuming apps (Zoom, FaceTime, …) read from, and a sink stream that
//  the terminal-cam CLI enqueues rendered frames into via the CoreMediaIO
//  C API. While no producer is attached, a placeholder frame is served.
//
//  UUIDs are fixed so the producer can find the device by UID. They must
//  match DEVICE_UID in src/vcam.rs.
//

import AppKit
import CoreMediaIO
import Foundation
import IOKit.audio
import os.log

let cameraName = "Terminal Cam"
let cameraDeviceUUID = UUID(uuidString: "72BC5AE8-7026-4057-90A9-A5BD0BDFDD95")!
let cameraSourceUUID = UUID(uuidString: "565A2280-4EFD-4807-A51B-B7F71C0C4C5A")!
let cameraSinkUUID = UUID(uuidString: "3A0F5A19-7BE8-4298-B837-809875974197")!

let cameraWidth = 1280
let cameraHeight = 720
let cameraFrameRate = 30

let log = OSLog(subsystem: "com.bluevisor.terminalcam.extension", category: "extension")

// MARK: - Provider

class CameraProviderSource: NSObject, CMIOExtensionProviderSource {
    private(set) var provider: CMIOExtensionProvider!
    private var deviceSource: CameraDeviceSource!

    init(clientQueue: DispatchQueue?) {
        super.init()
        provider = CMIOExtensionProvider(source: self, clientQueue: clientQueue)
        deviceSource = CameraDeviceSource(localizedName: cameraName)
        do {
            try provider.addDevice(deviceSource.device)
        } catch {
            fatalError("failed to add device: \(error.localizedDescription)")
        }
    }

    func connect(to client: CMIOExtensionClient) throws {}

    func disconnect(from client: CMIOExtensionClient) {}

    var availableProperties: Set<CMIOExtensionProperty> {
        [.providerName, .providerManufacturer]
    }

    func providerProperties(forProperties properties: Set<CMIOExtensionProperty>) throws
        -> CMIOExtensionProviderProperties
    {
        let providerProperties = CMIOExtensionProviderProperties(dictionary: [:])
        if properties.contains(.providerName) {
            providerProperties.name = cameraName
        }
        if properties.contains(.providerManufacturer) {
            providerProperties.manufacturer = "bluevisor"
        }
        return providerProperties
    }

    func setProviderProperties(_ providerProperties: CMIOExtensionProviderProperties) throws {}
}

// MARK: - Device

class CameraDeviceSource: NSObject, CMIOExtensionDeviceSource {
    private(set) var device: CMIOExtensionDevice!

    private var streamSource: CameraStreamSource!
    private var streamSink: CameraStreamSink!

    private var streamingCounter: UInt32 = 0
    private var sinkCounter: UInt32 = 0

    private var placeholderTimer: DispatchSourceTimer?
    private var consumeTimer: DispatchSourceTimer?

    private let timerQueue = DispatchQueue(
        label: "com.bluevisor.terminalcam.extension.timer",
        qos: .userInteractive,
        attributes: [],
        autoreleaseFrequency: .workItem,
        target: .global(qos: .userInteractive)
    )

    private var videoDescription: CMFormatDescription!
    private var bufferPool: CVPixelBufferPool!
    private var bufferAuxAttributes: NSDictionary!

    /// Pre-rendered placeholder frame (tight BGRA rows), served while no
    /// producer is feeding the sink.
    private var placeholderPixels: [UInt8] = []

    var sinkStarted = false

    init(localizedName: String) {
        super.init()

        device = CMIOExtensionDevice(
            localizedName: localizedName,
            deviceID: cameraDeviceUUID,
            legacyDeviceID: nil,
            source: self
        )

        CMVideoFormatDescriptionCreate(
            allocator: kCFAllocatorDefault,
            codecType: kCVPixelFormatType_32BGRA,
            width: Int32(cameraWidth),
            height: Int32(cameraHeight),
            extensions: nil,
            formatDescriptionOut: &videoDescription
        )

        let pixelBufferAttributes: NSDictionary = [
            kCVPixelBufferWidthKey: cameraWidth,
            kCVPixelBufferHeightKey: cameraHeight,
            kCVPixelBufferPixelFormatTypeKey: videoDescription.mediaSubType,
            kCVPixelBufferIOSurfacePropertiesKey: [CFString: CFTypeRef](),
        ]
        CVPixelBufferPoolCreate(kCFAllocatorDefault, nil, pixelBufferAttributes, &bufferPool)
        bufferAuxAttributes = [kCVPixelBufferPoolAllocationThresholdKey: 5]

        let format = CMIOExtensionStreamFormat(
            formatDescription: videoDescription,
            maxFrameDuration: CMTime(value: 1, timescale: Int32(cameraFrameRate)),
            minFrameDuration: CMTime(value: 1, timescale: Int32(cameraFrameRate)),
            validFrameDurations: nil
        )

        streamSource = CameraStreamSource(
            localizedName: "Terminal Cam Source",
            streamID: cameraSourceUUID,
            streamFormat: format,
            device: device
        )
        streamSink = CameraStreamSink(
            localizedName: "Terminal Cam Sink",
            streamID: cameraSinkUUID,
            streamFormat: format,
            device: device
        )

        do {
            try device.addStream(streamSource.stream)
            try device.addStream(streamSink.stream)
        } catch {
            fatalError("failed to add streams: \(error.localizedDescription)")
        }

        placeholderPixels = CameraDeviceSource.renderPlaceholder()
    }

    var availableProperties: Set<CMIOExtensionProperty> {
        [.deviceTransportType, .deviceModel]
    }

    func deviceProperties(forProperties properties: Set<CMIOExtensionProperty>) throws
        -> CMIOExtensionDeviceProperties
    {
        let deviceProperties = CMIOExtensionDeviceProperties(dictionary: [:])
        if properties.contains(.deviceTransportType) {
            deviceProperties.transportType = kIOAudioDeviceTransportTypeVirtual
        }
        if properties.contains(.deviceModel) {
            deviceProperties.model = "Terminal Cam"
        }
        return deviceProperties
    }

    func setDeviceProperties(_ deviceProperties: CMIOExtensionDeviceProperties) throws {}

    // MARK: Source-stream lifecycle

    func startStreaming() {
        guard bufferPool != nil else { return }

        streamingCounter += 1
        guard placeholderTimer == nil else { return }

        let timer = DispatchSource.makeTimerSource(flags: .strict, queue: timerQueue)
        timer.schedule(deadline: .now(), repeating: 1.0 / Double(cameraFrameRate))
        timer.setEventHandler { [weak self] in
            guard let self, !self.sinkStarted else { return }
            self.sendPlaceholderFrame()
        }
        placeholderTimer = timer
        timer.resume()
    }

    func stopStreaming() {
        if streamingCounter > 1 {
            streamingCounter -= 1
        } else {
            streamingCounter = 0
            placeholderTimer?.cancel()
            placeholderTimer = nil
        }
    }

    // MARK: Sink-stream lifecycle

    func startStreamingSink(client: CMIOExtensionClient) {
        sinkCounter += 1
        sinkStarted = true

        consumeTimer?.cancel()
        let timer = DispatchSource.makeTimerSource(flags: .strict, queue: timerQueue)
        // Drain faster than the producer's frame rate so the sink queue
        // (capacity 1) never backs up.
        timer.schedule(deadline: .now(), repeating: 1.0 / (Double(cameraFrameRate) * 3.0))
        timer.setEventHandler { [weak self] in
            self?.consumeBuffer(client)
        }
        consumeTimer = timer
        timer.resume()
    }

    func stopStreamingSink() {
        sinkStarted = false
        if sinkCounter > 1 {
            sinkCounter -= 1
        } else {
            sinkCounter = 0
            consumeTimer?.cancel()
            consumeTimer = nil
        }
    }

    private func consumeBuffer(_ client: CMIOExtensionClient) {
        guard sinkStarted else { return }

        streamSink.stream.consumeSampleBuffer(from: client) {
            [weak self] sampleBuffer, sequenceNumber, discontinuity, hasMoreSampleBuffers, error in
            guard let self, let sampleBuffer else { return }

            let nowNanoseconds = UInt64(
                CMClockGetTime(CMClockGetHostTimeClock()).seconds * Double(NSEC_PER_SEC)
            )
            if self.streamingCounter > 0 {
                self.streamSource.stream.send(
                    sampleBuffer,
                    discontinuity: [],
                    hostTimeInNanoseconds: UInt64(
                        sampleBuffer.presentationTimeStamp.seconds * Double(NSEC_PER_SEC)
                    )
                )
            }
            self.streamSink.stream.notifyScheduledOutputChanged(
                CMIOExtensionScheduledOutput(
                    sequenceNumber: sequenceNumber,
                    hostTimeInNanoseconds: nowNanoseconds
                )
            )
        }
    }

    // MARK: Placeholder frame

    private func sendPlaceholderFrame() {
        var pixelBuffer: CVPixelBuffer?
        let status = CVPixelBufferPoolCreatePixelBufferWithAuxAttributes(
            kCFAllocatorDefault, bufferPool, bufferAuxAttributes, &pixelBuffer
        )
        guard status == kCVReturnSuccess, let pixelBuffer else {
            if status == kCVReturnPoolAllocationFailed {
                os_log(.error, log: log, "placeholder: pixel buffer pool exhausted")
            }
            return
        }

        CVPixelBufferLockBaseAddress(pixelBuffer, [])
        if let base = CVPixelBufferGetBaseAddress(pixelBuffer) {
            let rowBytes = CVPixelBufferGetBytesPerRow(pixelBuffer)
            let tightRow = cameraWidth * 4
            self.placeholderPixels.withUnsafeBytes { src in
                let srcBase = src.baseAddress!
                if rowBytes == tightRow {
                    memcpy(base, srcBase, tightRow * cameraHeight)
                } else {
                    for row in 0..<cameraHeight {
                        memcpy(
                            base + row * rowBytes,
                            srcBase + row * tightRow,
                            tightRow
                        )
                    }
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(pixelBuffer, [])

        var timingInfo = CMSampleTimingInfo()
        timingInfo.presentationTimeStamp = CMClockGetTime(CMClockGetHostTimeClock())

        var sampleBuffer: CMSampleBuffer?
        let err = CMSampleBufferCreateForImageBuffer(
            allocator: kCFAllocatorDefault,
            imageBuffer: pixelBuffer,
            dataReady: true,
            makeDataReadyCallback: nil,
            refcon: nil,
            formatDescription: videoDescription,
            sampleTiming: &timingInfo,
            sampleBufferOut: &sampleBuffer
        )
        guard err == noErr, let sampleBuffer else { return }

        streamSource.stream.send(
            sampleBuffer,
            discontinuity: [],
            hostTimeInNanoseconds: UInt64(
                timingInfo.presentationTimeStamp.seconds * Double(NSEC_PER_SEC)
            )
        )
    }

    /// Draws the static "waiting for producer" frame once: dark background,
    /// green terminal-style title, gray hint line. Returns tight BGRA rows.
    private static func renderPlaceholder() -> [UInt8] {
        var pixels = [UInt8](repeating: 0, count: cameraWidth * cameraHeight * 4)
        pixels.withUnsafeMutableBytes { raw in
            guard
                let context = CGContext(
                    data: raw.baseAddress,
                    width: cameraWidth,
                    height: cameraHeight,
                    bitsPerComponent: 8,
                    bytesPerRow: cameraWidth * 4,
                    space: CGColorSpaceCreateDeviceRGB(),
                    bitmapInfo: CGImageAlphaInfo.noneSkipFirst.rawValue
                        | CGBitmapInfo.byteOrder32Little.rawValue
                )
            else { return }

            context.setFillColor(CGColor(red: 0.05, green: 0.06, blue: 0.05, alpha: 1))
            context.fill(CGRect(x: 0, y: 0, width: cameraWidth, height: cameraHeight))

            let graphicsContext = NSGraphicsContext(cgContext: context, flipped: false)
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = graphicsContext

            let title = NSAttributedString(
                string: "terminal-cam",
                attributes: [
                    .font: NSFont.monospacedSystemFont(ofSize: 64, weight: .semibold),
                    .foregroundColor: NSColor(red: 0.45, green: 0.95, blue: 0.55, alpha: 1),
                ]
            )
            let hint = NSAttributedString(
                string: "run `terminal-cam --virtual-cam` to start the feed",
                attributes: [
                    .font: NSFont.monospacedSystemFont(ofSize: 22, weight: .regular),
                    .foregroundColor: NSColor(white: 0.55, alpha: 1),
                ]
            )

            let titleSize = title.size()
            let hintSize = hint.size()
            title.draw(at: NSPoint(
                x: (CGFloat(cameraWidth) - titleSize.width) / 2,
                y: CGFloat(cameraHeight) / 2 + 10
            ))
            hint.draw(at: NSPoint(
                x: (CGFloat(cameraWidth) - hintSize.width) / 2,
                y: CGFloat(cameraHeight) / 2 - 10 - hintSize.height
            ))

            NSGraphicsContext.restoreGraphicsState()
        }
        return pixels
    }
}

// MARK: - Source stream

class CameraStreamSource: NSObject, CMIOExtensionStreamSource {
    private(set) var stream: CMIOExtensionStream!

    let device: CMIOExtensionDevice
    private let streamFormat: CMIOExtensionStreamFormat

    init(
        localizedName: String, streamID: UUID, streamFormat: CMIOExtensionStreamFormat,
        device: CMIOExtensionDevice
    ) {
        self.device = device
        self.streamFormat = streamFormat
        super.init()
        stream = CMIOExtensionStream(
            localizedName: localizedName,
            streamID: streamID,
            direction: .source,
            clockType: .hostTime,
            source: self
        )
    }

    var formats: [CMIOExtensionStreamFormat] { [streamFormat] }

    var activeFormatIndex = 0

    var availableProperties: Set<CMIOExtensionProperty> {
        [.streamActiveFormatIndex, .streamFrameDuration]
    }

    func streamProperties(forProperties properties: Set<CMIOExtensionProperty>) throws
        -> CMIOExtensionStreamProperties
    {
        let streamProperties = CMIOExtensionStreamProperties(dictionary: [:])
        if properties.contains(.streamActiveFormatIndex) {
            streamProperties.activeFormatIndex = 0
        }
        if properties.contains(.streamFrameDuration) {
            streamProperties.frameDuration = CMTime(value: 1, timescale: Int32(cameraFrameRate))
        }
        return streamProperties
    }

    func setStreamProperties(_ streamProperties: CMIOExtensionStreamProperties) throws {}

    func authorizedToStartStream(for client: CMIOExtensionClient) -> Bool {
        true
    }

    func startStream() throws {
        guard let deviceSource = device.source as? CameraDeviceSource else {
            fatalError("unexpected device source type")
        }
        deviceSource.startStreaming()
    }

    func stopStream() throws {
        guard let deviceSource = device.source as? CameraDeviceSource else {
            fatalError("unexpected device source type")
        }
        deviceSource.stopStreaming()
    }
}

// MARK: - Sink stream

class CameraStreamSink: NSObject, CMIOExtensionStreamSource {
    private(set) var stream: CMIOExtensionStream!

    let device: CMIOExtensionDevice
    private let streamFormat: CMIOExtensionStreamFormat
    private var client: CMIOExtensionClient?

    init(
        localizedName: String, streamID: UUID, streamFormat: CMIOExtensionStreamFormat,
        device: CMIOExtensionDevice
    ) {
        self.device = device
        self.streamFormat = streamFormat
        super.init()
        stream = CMIOExtensionStream(
            localizedName: localizedName,
            streamID: streamID,
            direction: .sink,
            clockType: .hostTime,
            source: self
        )
    }

    var formats: [CMIOExtensionStreamFormat] { [streamFormat] }

    var activeFormatIndex = 0

    var availableProperties: Set<CMIOExtensionProperty> {
        [
            .streamActiveFormatIndex,
            .streamFrameDuration,
            .streamSinkBufferQueueSize,
            .streamSinkBuffersRequiredForStartup,
            .streamSinkBufferUnderrunCount,
            .streamSinkEndOfData,
        ]
    }

    func streamProperties(forProperties properties: Set<CMIOExtensionProperty>) throws
        -> CMIOExtensionStreamProperties
    {
        let streamProperties = CMIOExtensionStreamProperties(dictionary: [:])
        if properties.contains(.streamActiveFormatIndex) {
            streamProperties.activeFormatIndex = 0
        }
        if properties.contains(.streamFrameDuration) {
            streamProperties.frameDuration = CMTime(value: 1, timescale: Int32(cameraFrameRate))
        }
        if properties.contains(.streamSinkBufferQueueSize) {
            streamProperties.sinkBufferQueueSize = 1
        }
        if properties.contains(.streamSinkBuffersRequiredForStartup) {
            streamProperties.sinkBuffersRequiredForStartup = 1
        }
        return streamProperties
    }

    func setStreamProperties(_ streamProperties: CMIOExtensionStreamProperties) throws {}

    func authorizedToStartStream(for client: CMIOExtensionClient) -> Bool {
        self.client = client
        return true
    }

    func startStream() throws {
        guard let deviceSource = device.source as? CameraDeviceSource else {
            fatalError("unexpected device source type")
        }
        if let client {
            deviceSource.startStreamingSink(client: client)
        }
    }

    func stopStream() throws {
        guard let deviceSource = device.source as? CameraDeviceSource else {
            fatalError("unexpected device source type")
        }
        deviceSource.stopStreamingSink()
    }
}
