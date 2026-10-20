#if canImport(MusicKit)

    import BridgethingCompanionCore
    import Combine
    import Foundation
    import ImageIO
    import MusicKit
    import os
    import SwiftUI
    import UniformTypeIdentifiers

    #if os(iOS)
        import AVFAudio
        import MediaPlayer
    #elseif os(macOS)
        import iTunesLibrary
    #endif

    private let musicLog = Logger(subsystem: "com.bridgething.companion", category: "applemusic")

    enum AmPlayerError: Error, CustomStringConvertible {
        case unavailable
        case itemNotFound(AmRef)
        case noCatalogSong(AmRef)
        case artworkUnknown(String)
        case artworkDidNotLoad(String)

        var description: String {
            switch self {
            case .unavailable: return "apple music playback is unavailable on this platform"
            case let .itemNotFound(ref): return "apple music item not found: \(ref.library ? "library " : "")\(ref.kind) \(ref.id)"
            case let .noCatalogSong(ref): return "apple music song has no catalog counterpart: \(ref.id)"
            case let .artworkUnknown(url): return "no musickit artwork is known for \(url)"
            case let .artworkDidNotLoad(url): return "musickit artwork did not load for \(url)"
            }
        }
    }

    protocol AmReply: AnyObject, Sendable {
        associatedtype Value
        func complete(value: Value)
        func fail(reason: String)
    }

    extension AmAuthSink: AmReply {}
    extension AmCatalogSink: AmReply {}
    extension AmPageSink: AmReply {}
    extension AmCountSink: AmReply {}
    extension AmItemsSink: AmReply {}
    extension AmItemSink: AmReply {}
    extension AmShelvesSink: AmReply {}
    extension AmSearchSink: AmReply {}
    extension AmFavoritesSink: AmReply {}
    extension AmRelationsSink: AmReply {}
    extension AmArtworkSink: AmReply {}

    private func answer<Sink: AmReply>(
        _ sink: Sink, _ work: @escaping @Sendable () async throws -> Sink.Value
    ) where Sink.Value: Sendable {
        Task {
            do {
                try await sink.complete(value: work())
            } catch {
                sink.fail(reason: String(describing: error))
            }
        }
    }

    private func act(_ sink: AmActionSink, _ work: @escaping @Sendable () async throws -> Void) {
        Task {
            do {
                try await work()
                sink.ok()
            } catch {
                sink.fail(reason: String(describing: error))
            }
        }
    }

    struct PlayParams: Decodable {
        let isLibrary: Bool?
        let catalogId: String?
    }

    func playParams(_ parameters: PlayParameters?) -> PlayParams? {
        guard let parameters, let data = try? JSONEncoder().encode(parameters) else { return nil }
        return try? JSONDecoder().decode(PlayParams.self, from: data)
    }

    private func catalogId(_ parameters: PlayParameters?) -> String? {
        playParams(parameters)?.catalogId
    }

    extension AmRef {
        static func held(_ kind: AmKind, _ id: MusicItemID, library: Bool) -> AmRef {
            AmRef(kind: kind, id: id.rawValue, library: library)
        }

        static func transient(_ kind: AmKind, _ id: MusicItemID, _ parameters: PlayParameters?) -> AmRef {
            guard let params = playParams(parameters), params.isLibrary == true else {
                return AmRef(kind: kind, id: id.rawValue, library: false)
            }
            if let catalogId = params.catalogId {
                return AmRef(kind: kind, id: catalogId, library: false)
            }
            return AmRef(kind: kind, id: id.rawValue, library: true)
        }
    }

    private let artSentinel = 999_999_999
    private let defaultRenderEdge = 600

    final class ArtworkRegistry: @unchecked Sendable {
        static let shared = ArtworkRegistry()
        private static let capacity = 2048

        private let lock = NSLock()
        private var byTemplate: [String: Artwork] = [:]
        private var order: [String] = []

        func remember(_ template: String, _ artwork: Artwork) {
            lock.withLock {
                if byTemplate.updateValue(artwork, forKey: template) == nil {
                    order.append(template)
                }
                while order.count > Self.capacity {
                    byTemplate.removeValue(forKey: order.removeFirst())
                }
            }
        }

        func lookup(_ url: String) -> (artwork: Artwork, edge: Int)? {
            lock.withLock {
                for match in url.matches(of: #/(\d+)x(\d+)/#).reversed() {
                    let template = url.replacingCharacters(in: match.range, with: "{w}x{h}")
                    if let artwork = byTemplate[template], let edge = Int(match.output.1) {
                        return (artwork, edge)
                    }
                }
                return byTemplate[url].map { ($0, min(defaultRenderEdge, $0.maximumWidth)) }
            }
        }
    }

    func artworkTemplate(_ artwork: Artwork?) -> String? {
        guard let artwork else { return nil }
        guard let url = artwork.url(width: artSentinel, height: artSentinel) else { return nil }
        let sentinel = "\(artSentinel)x\(artSentinel)"
        let raw = url.absoluteString
        let template = raw.contains(sentinel) ? raw.replacingOccurrences(of: sentinel, with: "{w}x{h}") : raw
        ArtworkRegistry.shared.remember(template, artwork)
        return template
    }

    @MainActor
    private final class RenderWait {
        private var continuation: CheckedContinuation<Bool, Never>?
        private var watch: AnyCancellable?

        func until<Content: View>(_ renderer: ImageRenderer<Content>, within deadline: Duration) async -> Bool {
            await withCheckedContinuation { continuation in
                self.continuation = continuation
                watch = renderer.objectWillChange.sink { [weak self] _ in
                    Task { @MainActor [weak self] in self?.finish(true) }
                }
                _ = renderer.cgImage
                Task { @MainActor [weak self] in
                    try? await Task.sleep(for: deadline)
                    self?.finish(false)
                }
            }
        }

        private func finish(_ changed: Bool) {
            watch = nil
            continuation?.resume(returning: changed)
            continuation = nil
        }
    }

    enum ArtworkRendering {
        static let loadDeadline: Duration = .seconds(5)

        @MainActor
        static func render(_ artwork: Artwork, edge: Int, url: String) async throws -> Data {
            let side = CGFloat(edge)
            let renderer = ImageRenderer(content: ArtworkImage(artwork, width: side, height: side))
            renderer.scale = 1
            guard await RenderWait().until(renderer, within: loadDeadline) else {
                throw AmPlayerError.artworkDidNotLoad(url)
            }
            try? await Task.sleep(for: .milliseconds(10))
            guard let image = renderer.cgImage else { throw AmPlayerError.artworkDidNotLoad(url) }
            return try jpeg(image, url: url)
        }

        private static func jpeg(_ image: CGImage, url: String) throws -> Data {
            let data = NSMutableData()
            guard let destination = CGImageDestinationCreateWithData(data, UTType.jpeg.identifier as CFString, 1, nil)
            else { throw AmPlayerError.artworkDidNotLoad(url) }
            CGImageDestinationAddImage(destination, image, [kCGImageDestinationLossyCompressionQuality: 0.9] as CFDictionary)
            guard CGImageDestinationFinalize(destination) else { throw AmPlayerError.artworkDidNotLoad(url) }
            return data as Data
        }
    }

    enum MusicKitItems {
        static func resolve<T>(
            _ ref: AmRef,
            catalogFilter: KeyPath<T.FilterType, MusicItemID>,
            libraryFilter: KeyPath<T.LibraryFilter, MusicItemID>
        ) async throws -> T where T: FilterableMusicItem & MusicLibraryRequestable & Decodable {
            if ref.library {
                var req = MusicLibraryRequest<T>()
                req.filter(matching: libraryFilter, equalTo: MusicItemID(ref.id))
                guard let item = try await req.response().items.first else { throw AmPlayerError.itemNotFound(ref) }
                return item
            }
            let req = MusicCatalogResourceRequest<T>(matching: catalogFilter, equalTo: MusicItemID(ref.id))
            guard let item = try await req.response().items.first else { throw AmPlayerError.itemNotFound(ref) }
            return item
        }

        static func song(_ ref: AmRef) async throws -> Song {
            try await resolve(ref, catalogFilter: \.id, libraryFilter: \.id)
        }

        static func album(_ ref: AmRef) async throws -> Album {
            try await resolve(ref, catalogFilter: \.id, libraryFilter: \.id)
        }

        static func playlist(_ ref: AmRef) async throws -> Playlist {
            try await resolve(ref, catalogFilter: \.id, libraryFilter: \.id)
        }

        static func artist(_ ref: AmRef) async throws -> Artist {
            try await resolve(ref, catalogFilter: \.id, libraryFilter: \.id)
        }

        static func station(_ ref: AmRef) async throws -> Station {
            guard !ref.library else { throw AmPlayerError.itemNotFound(ref) }
            let req = MusicCatalogResourceRequest<Station>(matching: \.id, equalTo: MusicItemID(ref.id))
            guard let station = try await req.response().items.first else { throw AmPlayerError.itemNotFound(ref) }
            return station
        }

        static func librarySongs() async throws -> [Song] {
            var req = MusicLibraryRequest<Song>()
            #if os(iOS)
                req.sort(by: \.libraryAddedDate, ascending: false)
            #endif
            return try await everything(req.response().items)
        }

        static func everything<T>(_ first: MusicItemCollection<T>?) async throws -> [T] where T: Decodable {
            guard var batch = first else { return [] }
            var all = Array(batch)
            while batch.hasNextBatch, let next = try await batch.nextBatch() {
                all.append(contentsOf: next)
                batch = next
            }
            return all
        }
    }

    private let releaseCalendar: Calendar = {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(secondsFromGMT: 0) ?? .current
        return calendar
    }()

    func amDate(_ date: Date?) -> AmDate? {
        guard let date else { return nil }
        let parts = releaseCalendar.dateComponents([.year, .month, .day], from: date)
        guard let year = parts.year, let month = parts.month, let day = parts.day else { return nil }
        return AmDate(year: Int32(year), month: UInt32(month), day: UInt32(day))
    }

    func amItem(_ song: Song, as ref: AmRef) -> AmItem {
        AmItem(
            item: ref, title: song.title, subtitle: nil,
            artistName: song.artistName, albumName: song.albumTitle,
            artworkUrl: artworkTemplate(song.artwork),
            durationMs: song.duration.map { UInt32(($0 * 1000).rounded()) },
            trackCount: nil, released: amDate(song.releaseDate), single: nil
        )
    }

    func amItem(_ album: Album, as ref: AmRef) -> AmItem {
        AmItem(
            item: ref, title: album.title, subtitle: album.artistName,
            artistName: album.artistName, albumName: nil,
            artworkUrl: artworkTemplate(album.artwork), durationMs: nil,
            trackCount: UInt32(album.trackCount), released: amDate(album.releaseDate), single: album.isSingle
        )
    }

    func amItem(_ artist: Artist, as ref: AmRef) -> AmItem {
        AmItem(
            item: ref, title: artist.name, subtitle: nil, artistName: nil, albumName: nil,
            artworkUrl: artworkTemplate(artist.artwork), durationMs: nil, trackCount: nil, released: nil, single: nil
        )
    }

    func amItem(_ playlist: Playlist, as ref: AmRef) -> AmItem {
        AmItem(
            item: ref, title: playlist.name, subtitle: playlist.curatorName, artistName: nil, albumName: nil,
            artworkUrl: artworkTemplate(playlist.artwork), durationMs: nil, trackCount: nil, released: nil, single: nil
        )
    }

    func amItem(_ station: Station, as ref: AmRef) -> AmItem {
        AmItem(
            item: ref, title: station.name, subtitle: nil, artistName: nil, albumName: nil,
            artworkUrl: artworkTemplate(station.artwork), durationMs: nil, trackCount: nil, released: nil, single: nil
        )
    }

    private let librarySongsTitle = "Library Songs"

    func librarySongsItem(_ ref: AmRef) -> AmItem {
        AmItem(
            item: ref, title: librarySongsTitle, subtitle: nil, artistName: nil, albumName: nil,
            artworkUrl: nil, durationMs: nil, trackCount: nil, released: nil, single: nil
        )
    }

    func amItem(_ song: Song, library: Bool) -> AmItem {
        amItem(song, as: .held(.song, song.id, library: library))
    }

    func amItem(_ album: Album, library: Bool) -> AmItem {
        amItem(album, as: .held(.album, album.id, library: library))
    }

    func amItem(_ artist: Artist, library: Bool) -> AmItem {
        amItem(artist, as: .held(.artist, artist.id, library: library))
    }

    func amItem(_ playlist: Playlist, library: Bool) -> AmItem {
        amItem(playlist, as: .held(.playlist, playlist.id, library: library))
    }

    func amItem(_ station: Station, library: Bool) -> AmItem {
        amItem(station, as: .held(.station, station.id, library: library))
    }

    private func transientItem(_ album: Album) -> AmItem {
        amItem(album, as: .transient(.album, album.id, album.playParameters))
    }

    private func transientItem(_ playlist: Playlist) -> AmItem {
        amItem(playlist, as: .transient(.playlist, playlist.id, playlist.playParameters))
    }

    final class MusicKitLibrary: @unchecked Sendable {
        private static let favoritesBatchSize = 300

        private let storefrontLock = NSLock()
        private var cachedStorefront: String?

        func page(_ scope: AmLibraryScope, limit: UInt32, offset: UInt32) async throws -> AmPage {
            switch scope {
            case .playlists:
                var req = MusicLibraryRequest<Playlist>()
                req.limit = Int(limit)
                req.offset = Int(offset)
                #if os(iOS)
                    req.sort(by: \.lastPlayedDate, ascending: false)
                #endif
                let items = try await req.response().items
                return AmPage(items: items.map { amItem($0, library: true) }, hasMore: items.hasNextBatch)
            case .albums:
                var req = MusicLibraryRequest<Album>()
                req.limit = Int(limit)
                req.offset = Int(offset)
                #if os(iOS)
                    req.sort(by: \.libraryAddedDate, ascending: false)
                #endif
                let items = try await req.response().items
                return AmPage(items: items.map { amItem($0, library: true) }, hasMore: items.hasNextBatch)
            case .artists:
                var req = MusicLibraryRequest<Artist>()
                req.limit = Int(limit)
                req.offset = Int(offset)
                let items = try await req.response().items
                return AmPage(items: items.map { amItem($0, library: true) }, hasMore: items.hasNextBatch)
            case .songs:
                var req = MusicLibraryRequest<Song>()
                req.limit = Int(limit)
                req.offset = Int(offset)
                #if os(iOS)
                    req.sort(by: \.libraryAddedDate, ascending: false)
                #endif
                let items = try await req.response().items
                return AmPage(items: items.map { amItem($0, library: true) }, hasMore: items.hasNextBatch)
            }
        }

        func count(_ scope: AmLibraryScope) async throws -> UInt32 {
            switch scope {
            case .playlists: return try await Self.count(Playlist.self)
            case .albums: return try await Self.count(Album.self)
            case .artists: return try await Self.count(Artist.self)
            case .songs: return try await Self.count(Song.self)
            }
        }

        private static func count<T>(_: T.Type) async throws -> UInt32 where T: MusicLibraryRequestable & Decodable {
            var batch = try await MusicLibraryRequest<T>().response().items
            var total = batch.count
            while batch.hasNextBatch, let next = try await batch.nextBatch() {
                total += next.count
                batch = next
            }
            return UInt32(clamping: total)
        }

        func recentlyPlayed(limit: UInt32, offset: UInt32) async throws -> AmPage {
            var req = MusicRecentlyPlayedContainerRequest()
            req.limit = Int(limit)
            req.offset = Int(offset)
            let items = try await req.response().items
            let mapped: [AmItem] = items.compactMap { item in
                switch item {
                case let .album(album): return transientItem(album)
                case let .playlist(playlist): return transientItem(playlist)
                case let .station(station): return amItem(station, library: false)
                @unknown default: return nil
                }
            }
            return AmPage(items: mapped, hasMore: items.hasNextBatch)
        }

        func children(of ref: AmRef) async throws -> [AmItem] {
            switch ref.kind {
            case .album:
                let album = try await MusicKitItems.album(ref).with([.tracks])
                return try await Self.songs(MusicKitItems.everything(album.tracks), library: ref.library)
            case .playlist:
                let playlist = try await MusicKitItems.playlist(ref).with([.tracks])
                return try await Self.songs(MusicKitItems.everything(playlist.tracks), library: ref.library)
            case .artist where ref.library:
                let artist = try await MusicKitItems.artist(ref).with([.albums])
                return try await MusicKitItems.everything(artist.albums).map { amItem($0, library: true) }
            case .artist:
                let artist = try await MusicKitItems.artist(ref).with([.topSongs, .albums])
                let top = (artist.topSongs ?? []).map { amItem($0, library: false) }
                let albums = try await MusicKitItems.everything(artist.albums).map { amItem($0, library: false) }
                return top + albums
            case .songs:
                return try await MusicKitItems.librarySongs().map { amItem($0, library: true) }
            case .song, .station:
                throw AmPlayerError.itemNotFound(ref)
            }
        }

        func relations(of ref: AmRef) async throws -> AmRelations {
            switch ref.kind {
            case .song:
                let song = try await MusicKitItems.song(ref).with([.albums, .artists, .station])
                return AmRelations(
                    album: song.albums?.first.map { .held(.album, $0.id, library: ref.library) },
                    artist: song.artists?.first.map { .held(.artist, $0.id, library: ref.library) },
                    station: song.station.map { .held(.station, $0.id, library: false) }
                )
            case .artist:
                let artist = try await MusicKitItems.artist(ref).with([.station])
                return AmRelations(
                    album: nil, artist: nil,
                    station: artist.station.map { .held(.station, $0.id, library: false) }
                )
            case .album, .playlist, .station, .songs:
                return AmRelations(album: nil, artist: nil, station: nil)
            }
        }

        private static func songs(_ tracks: [MusicKit.Track], library: Bool) -> [AmItem] {
            tracks.compactMap { track in
                guard case let .song(song) = track else { return nil }
                return amItem(song, library: library)
            }
        }

        func recommendations() async throws -> [AmShelf] {
            let res = try await MusicPersonalRecommendationsRequest().response()
            return res.recommendations.map { rec in
                let items: [AmItem] = rec.items.compactMap { item in
                    switch item {
                    case let .album(album): return transientItem(album)
                    case let .playlist(playlist): return transientItem(playlist)
                    case let .station(station): return amItem(station, library: false)
                    @unknown default: return nil
                    }
                }
                return AmShelf(id: rec.id.rawValue, title: rec.title ?? "For You", items: items, total: UInt32(items.count))
            }
        }

        func resolve(_ ref: AmRef) async throws -> AmItem {
            switch ref.kind {
            case .song: return try await amItem(MusicKitItems.song(ref), as: ref)
            case .album: return try await amItem(MusicKitItems.album(ref), as: ref)
            case .playlist: return try await amItem(MusicKitItems.playlist(ref), as: ref)
            case .artist: return try await amItem(MusicKitItems.artist(ref), as: ref)
            case .station: return try await amItem(MusicKitItems.station(ref), as: ref)
            case .songs: return librarySongsItem(ref)
            }
        }

        func search(_ query: String, limit: UInt32, offset: UInt32) async throws -> AmSearchResults {
            var req = MusicCatalogSearchRequest(
                term: query, types: [Song.self, Album.self, Artist.self, Playlist.self, Station.self]
            )
            req.limit = Int(limit)
            req.offset = Int(offset)
            req.includeTopResults = offset == 0
            let res = try await req.response()
            let top: [AmItem] = res.topResults.compactMap { result in
                switch result {
                case let .song(song): return amItem(song, library: false)
                case let .album(album): return amItem(album, library: false)
                case let .artist(artist): return amItem(artist, library: false)
                case let .playlist(playlist): return amItem(playlist, library: false)
                case let .station(station): return amItem(station, library: false)
                default: return nil
                }
            }
            return AmSearchResults(
                top: top,
                songs: res.songs.map { amItem($0, library: false) },
                albums: res.albums.map { amItem($0, library: false) },
                artists: res.artists.map { amItem($0, library: false) },
                playlists: res.playlists.map { amItem($0, library: false) },
                stations: res.stations.map { amItem($0, library: false) }
            )
        }

        private func storefront() async throws -> String {
            if let cached = storefrontLock.withLock({ cachedStorefront }) { return cached }
            let code = try await MusicDataRequest.currentCountryCode
            storefrontLock.withLock { cachedStorefront = code }
            return code
        }

        private func catalogSongId(_ ref: AmRef) async -> String? {
            guard ref.kind == .song else { return nil }
            guard ref.library else { return ref.id }
            guard let song = try? await MusicKitItems.song(ref) else { return nil }
            return catalogId(song.playParameters)
        }

        func isFavorite(_ refs: [AmRef]) async -> [Bool] {
            var catalogIds: [String?] = []
            for ref in refs {
                await catalogIds.append(catalogSongId(ref))
            }
            let wanted = Array(Set(catalogIds.compactMap { $0 }))
            var byId: [String: Bool] = [:]
            for chunk in stride(from: 0, to: wanted.count, by: Self.favoritesBatchSize) {
                let ids = Array(wanted[chunk ..< min(chunk + Self.favoritesBatchSize, wanted.count)])
                for (id, favorite) in await favoriteFlags(ids: ids) {
                    byId[id] = favorite
                }
            }
            return catalogIds.map { id in id.flatMap { byId[$0] } ?? false }
        }

        private func favoriteFlags(ids: [String]) async -> [String: Bool] {
            do {
                let sf = try await storefront()
                var comps = URLComponents(string: "https://api.music.apple.com/v1/catalog/\(sf)/songs")!
                comps.queryItems = [
                    URLQueryItem(name: "ids", value: ids.joined(separator: ",")),
                    URLQueryItem(name: "extend", value: "inFavorites"),
                ]
                let response = try await MusicDataRequest(urlRequest: URLRequest(url: comps.url!)).response()
                let decoded = try JSONDecoder().decode(InFavoritesResponse.self, from: response.data)
                return decoded.data.reduce(into: [:]) { acc, entry in
                    acc[entry.id] = entry.attributes?.inFavorites ?? false
                }
            } catch {
                musicLog.notice("favorites lookup failed: \(String(describing: error), privacy: .public)")
                return [:]
            }
        }

        func addFavorite(_ ref: AmRef) async throws {
            guard let id = await catalogSongId(ref) else { throw AmPlayerError.noCatalogSong(ref) }
            var comps = URLComponents(string: "https://api.music.apple.com/v1/me/favorites")!
            comps.queryItems = [URLQueryItem(name: "ids[songs]", value: id)]
            var urlRequest = URLRequest(url: comps.url!)
            urlRequest.httpMethod = "POST"
            _ = try await MusicDataRequest(urlRequest: urlRequest).response()
        }

        func artwork(_ url: String) async throws -> Data {
            do {
                return try await Self.fetchShared(url)
            } catch {
                musicLog.notice(
                    "artwork session fetch failed, rendering instead: \(String(describing: error), privacy: .public)"
                )
            }
            guard let known = ArtworkRegistry.shared.lookup(url) else { throw AmPlayerError.artworkUnknown(url) }
            return try await ArtworkRendering.render(known.artwork, edge: known.edge, url: url)
        }

        private static func fetchShared(_ url: String) async throws -> Data {
            guard let target = URL(string: url) else { throw AmPlayerError.artworkDidNotLoad(url) }
            let (data, response) = try await URLSession.shared.data(from: target)
            if let http = response as? HTTPURLResponse, !(200 ..< 300).contains(http.statusCode) {
                throw AmPlayerError.artworkDidNotLoad(url)
            }
            guard !data.isEmpty else { throw AmPlayerError.artworkDidNotLoad(url) }
            return data
        }

        private struct InFavoritesResponse: Decodable {
            struct Entry: Decodable {
                struct Attributes: Decodable { let inFavorites: Bool? }
                let id: String
                let attributes: Attributes?
            }

            let data: [Entry]
        }
    }

    public final class MusicKitBackend: AppleMusicBackend, @unchecked Sendable {
        private static let libraryChangeSettle: Duration = .seconds(1)

        private let lock = NSLock()
        private var observeTask: Task<Void, Never>?
        private var libraryObserver: NSObjectProtocol?
        private var librarySettle: Task<Void, Never>?
        private let library = MusicKitLibrary()

        public init() {}

        public func start(inbox: AmInbox) {
            stop()
            #if os(iOS)
                let (stream, cont) = AsyncStream<Void>.makeStream()
                Self.observePlayer(into: cont)
                let task = Task {
                    await withTaskCancellationHandler {
                        for await _ in stream {
                            if Task.isCancelled { break }
                            inbox.onPlayerChanged()
                        }
                    } onCancel: {
                        cont.finish()
                    }
                }
                lock.withLock { observeTask = task }
            #endif
            let observer = Self.observeLibrary { [weak self] in self?.libraryChanged(inbox) }
            lock.withLock { libraryObserver = observer }
        }

        public func stop() {
            let (task, observer, settle) = lock.withLock {
                defer {
                    observeTask = nil
                    libraryObserver = nil
                    librarySettle = nil
                }
                return (observeTask, libraryObserver, librarySettle)
            }
            task?.cancel()
            settle?.cancel()
            if let observer {
                Self.stopObservingLibrary(observer)
            }
        }

        private func libraryChanged(_ inbox: AmInbox) {
            let settle = Task {
                try? await Task.sleep(for: Self.libraryChangeSettle)
                if Task.isCancelled { return }
                inbox.onLibraryChanged()
            }
            lock.withLock { librarySettle.replace(with: settle) }?.cancel()
        }

        #if os(iOS)
            private static func observeLibrary(_ changed: @escaping @Sendable () -> Void) -> NSObjectProtocol {
                MPMediaLibrary.default().beginGeneratingLibraryChangeNotifications()
                return NotificationCenter.default.addObserver(
                    forName: .MPMediaLibraryDidChange, object: nil, queue: nil
                ) { _ in changed() }
            }

            private static func stopObservingLibrary(_ observer: NSObjectProtocol) {
                NotificationCenter.default.removeObserver(observer)
                MPMediaLibrary.default().endGeneratingLibraryChangeNotifications()
            }

            private static func observePlayer(into cont: AsyncStream<Void>.Continuation) {
                let player = SystemMusicPlayer.shared
                let stateSub = player.state.objectWillChange.sink { _ in
                    Task {
                        try? await Task.sleep(for: .milliseconds(80))
                        cont.yield(())
                    }
                }
                let queueSub = player.queue.objectWillChange.sink { _ in
                    Task {
                        try? await Task.sleep(for: .milliseconds(80))
                        cont.yield(())
                    }
                }
                nonisolated(unsafe) let box = [stateSub, queueSub]
                cont.onTermination = { _ in _ = box }
            }
        #else
            private static func observeLibrary(_ changed: @escaping @Sendable () -> Void) -> NSObjectProtocol {
                DistributedNotificationCenter.default().addObserver(
                    forName: .ITLibraryDidChange, object: nil, queue: nil
                ) { _ in changed() }
            }

            private static func stopObservingLibrary(_ observer: NSObjectProtocol) {
                DistributedNotificationCenter.default().removeObserver(observer)
            }
        #endif

        public func snapshot(sink: AmSnapshotSink) {
            #if os(iOS)
                Task { @MainActor in
                    let player = SystemMusicPlayer.shared
                    let amEntry: AmEntry? = player.queue.currentEntry.map { entry in
                        var ref: AmRef?
                        var durationMs: UInt32?
                        switch entry.item {
                        case let .song(song):
                            ref = .transient(.song, song.id, song.playParameters)
                            durationMs = song.duration.map { UInt32(($0 * 1000).rounded()) }
                        case let .musicVideo(video):
                            durationMs = video.duration.map { UInt32(($0 * 1000).rounded()) }
                        default:
                            break
                        }
                        return AmEntry(
                            item: ref,
                            title: entry.title,
                            artistName: entry.subtitle,
                            albumName: nil,
                            artworkUrl: artworkTemplate(entry.artwork),
                            durationMs: durationMs
                        )
                    }
                    let state = player.state
                    let repeatMode: AmRepeatMode = switch state.repeatMode {
                    case .one: .one
                    case .all: .all
                    default: .off
                    }
                    sink.complete(value: AmPlayerSnapshot(
                        entry: amEntry,
                        playing: state.playbackStatus == .playing,
                        positionMs: UInt32(max(0, player.playbackTime * 1000).rounded()),
                        shuffle: state.shuffleMode == .songs,
                        repeat: repeatMode,
                        canSeek: true
                    ))
                }
            #else
                sink.complete(value: AmPlayerSnapshot(
                    entry: nil, playing: false, positionMs: 0, shuffle: false, repeat: .off, canSeek: true
                ))
            #endif
        }

        private static func mapAuth(_ status: MusicAuthorization.Status) -> AmAuthStatus {
            switch status {
            case .notDetermined: return .notDetermined
            case .authorized: return .authorized
            case .denied: return .denied
            case .restricted: return .restricted
            @unknown default: return .denied
            }
        }

        public func authStatus(sink: AmAuthSink) {
            sink.complete(value: Self.mapAuth(MusicAuthorization.currentStatus))
        }

        public func requestAuthorization(sink: AmAuthSink) {
            answer(sink) { Self.mapAuth(await MusicAuthorization.request()) }
        }

        public func canPlayCatalogContent(sink: AmCatalogSink) {
            answer(sink) { try? await MusicSubscription.current.canPlayCatalogContent }
        }

        public func isOtherAudioPlaying(sink: AmFlagSink) {
            #if os(iOS)
                sink.complete(value: AVAudioSession.sharedInstance().isOtherAudioPlaying)
            #else
                sink.complete(value: false)
            #endif
        }

        public func playContext(context: AmRef, startAt: AmRef?, sink: AmActionSink) {
            #if os(iOS)
                act(sink) { try await Self.playContext(context, startAt: startAt) }
            #else
                sink.fail(reason: AmPlayerError.unavailable.description)
            #endif
        }

        public func queueInsert(item: AmRef, next: Bool, sink: AmActionSink) {
            #if os(iOS)
                act(sink) {
                    guard item.kind == .song else { throw AmPlayerError.itemNotFound(item) }
                    let song = try await MusicKitItems.song(item)
                    try await SystemMusicPlayer.shared.queue.insert(song, position: next ? .afterCurrentEntry : .tail)
                }
            #else
                sink.fail(reason: AmPlayerError.unavailable.description)
            #endif
        }

        public func command(cmd: AmPlayerCommand, sink: AmActionSink) {
            #if os(iOS)
                act(sink) {
                    let player = SystemMusicPlayer.shared
                    switch cmd {
                    case .play: try await player.play()
                    case .pause: player.pause()
                    case .skipNext: try await player.skipToNextEntry()
                    case .skipPrev: try await player.skipToPreviousEntry()
                    case let .seekTo(positionMs): player.playbackTime = TimeInterval(positionMs) / 1000
                    case let .setShuffle(on): player.state.shuffleMode = on ? .songs : .off
                    case let .setRepeat(mode):
                        player.state.repeatMode = switch mode {
                        case .off: MusicPlayer.RepeatMode.none
                        case .all: .all
                        case .one: .one
                        }
                    }
                }
            #else
                sink.fail(reason: AmPlayerError.unavailable.description)
            #endif
        }

        #if os(iOS)
            private static func playContext(_ context: AmRef, startAt: AmRef?) async throws {
                let player = SystemMusicPlayer.shared
                switch context.kind {
                case .song:
                    player.queue = try await [MusicKitItems.song(context)]
                case .album:
                    let album = try await MusicKitItems.album(context)
                    let tracks = try await MusicKitItems.everything(album.with([.tracks]).tracks)
                    if let startAt, let start = tracks.first(where: { $0.id.rawValue == startAt.id }) {
                        player.queue = SystemMusicPlayer.Queue(for: tracks, startingAt: start)
                    } else {
                        player.queue = [album]
                    }
                case .playlist:
                    let playlist = try await MusicKitItems.playlist(context)
                    let tracks = try await MusicKitItems.everything(playlist.with([.tracks]).tracks)
                    if let startAt, let start = tracks.first(where: { $0.id.rawValue == startAt.id }) {
                        player.queue = SystemMusicPlayer.Queue(for: tracks, startingAt: start)
                    } else {
                        player.queue = [playlist]
                    }
                case .artist where context.library:
                    let artist = try await MusicKitItems.artist(context)
                    let albums = try await MusicKitItems.everything(artist.with([.albums]).albums)
                    guard !albums.isEmpty else { throw AmPlayerError.itemNotFound(context) }
                    player.queue = SystemMusicPlayer.Queue(for: albums, startingAt: nil)
                case .artist:
                    let artist = try await MusicKitItems.artist(context)
                    guard let top = try await artist.with([.topSongs]).topSongs, !top.isEmpty else {
                        throw AmPlayerError.itemNotFound(context)
                    }
                    let start = startAt.flatMap { startAt in top.first { $0.id.rawValue == startAt.id } }
                    player.queue = SystemMusicPlayer.Queue(for: top, startingAt: start)
                case .station:
                    player.queue = try await [MusicKitItems.station(context)]
                case .songs:
                    let songs = try await MusicKitItems.librarySongs()
                    guard !songs.isEmpty else { throw AmPlayerError.itemNotFound(context) }
                    let start = startAt.flatMap { startAt in songs.first { $0.id.rawValue == startAt.id } }
                    player.queue = SystemMusicPlayer.Queue(for: songs, startingAt: start)
                }
                try await player.play()
            }
        #endif

        public func library(scope: AmLibraryScope, limit: UInt32, offset: UInt32, sink: AmPageSink) {
            answer(sink) { [library] in try await library.page(scope, limit: limit, offset: offset) }
        }

        public func libraryCount(scope: AmLibraryScope, sink: AmCountSink) {
            answer(sink) { [library] in try await library.count(scope) }
        }

        public func recentlyPlayed(limit: UInt32, offset: UInt32, sink: AmPageSink) {
            answer(sink) { [library] in try await library.recentlyPlayed(limit: limit, offset: offset) }
        }

        public func children(item: AmRef, sink: AmItemsSink) {
            answer(sink) { [library] in try await library.children(of: item) }
        }

        public func recommendations(sink: AmShelvesSink) {
            answer(sink) { [library] in try await library.recommendations() }
        }

        public func resolve(item: AmRef, sink: AmItemSink) {
            answer(sink) { [library] in try await library.resolve(item) }
        }

        public func relations(item: AmRef, sink: AmRelationsSink) {
            answer(sink) { [library] in try await library.relations(of: item) }
        }

        public func search(query: String, limit: UInt32, offset: UInt32, sink: AmSearchSink) {
            answer(sink) { [library] in try await library.search(query, limit: limit, offset: offset) }
        }

        public func isFavorite(items: [AmRef], sink: AmFavoritesSink) {
            answer(sink) { [library] in await library.isFavorite(items) }
        }

        public func addFavorite(item: AmRef, sink: AmActionSink) {
            act(sink) { [library] in try await library.addFavorite(item) }
        }

        public func artwork(url: String, sink: AmArtworkSink) {
            answer(sink) { [library] in try await library.artwork(url) }
        }
    }

    private extension Optional {
        mutating func replace(with value: Wrapped) -> Wrapped? {
            let previous = self
            self = value
            return previous
        }
    }

#endif
