#if canImport(MusicKit)

    import BridgethingCompanionCore
    import Foundation
    import ImageIO
    import MusicKit
    import XCTest

    @testable import BridgethingCompanion

    final class AppleMusicLiveTests: XCTestCase {
        private static let heroEdge = 248

        private func requireLive() throws -> MusicKitLibrary {
            try XCTSkipUnless(
                ProcessInfo.processInfo.environment["BRIDGETHING_APPLEMUSIC_LIVE"] == "1",
                "set BRIDGETHING_APPLEMUSIC_LIVE=1 to run live Apple Music validation"
            )
            return MusicKitLibrary()
        }

        private func requireAuthorized() throws -> MusicKitLibrary {
            let library = try requireLive()
            let status = MusicAuthorization.currentStatus
            guard status == .authorized else {
                XCTFail("media library access is \(status); run scripts/apple-music-authorize once from this terminal")
                throw AmPlayerError.unavailable
            }
            return library
        }

        private func requireLibrary() throws -> MusicKitLibrary {
            try XCTSkipUnless(
                ProcessInfo.processInfo.environment["BRIDGETHING_APPLEMUSIC_LIBRARY"] == "1",
                "set BRIDGETHING_APPLEMUSIC_LIBRARY=1 to run the cases that read the signed-in music library"
            )
            return try requireAuthorized()
        }

        private func catalog<T>(_ what: String, _ work: () async throws -> T) async throws -> T {
            do {
                return try await work()
            } catch {
                let text = String(describing: error).lowercased()
                if text.contains("token") || text.contains("401") || text.contains("403") {
                    throw XCTSkip("\(what) needs a developer token the unsigned test host cannot mint: \(error)")
                }
                throw error
            }
        }

        private func sized(_ template: String, _ edge: Int) -> String {
            template.replacingOccurrences(of: "{w}", with: "\(edge)").replacingOccurrences(of: "{h}", with: "\(edge)")
        }

        private func decodedImage(_ data: Data) throws -> CGImage {
            let source = try XCTUnwrap(CGImageSourceCreateWithData(data as CFData, nil))
            return try XCTUnwrap(CGImageSourceCreateImageAtIndex(source, 0, nil))
        }

        private func distinctColors(_ image: CGImage) throws -> Int {
            let width = image.width
            let height = image.height
            var pixels = [UInt8](repeating: 0, count: width * height * 4)
            let context = try XCTUnwrap(CGContext(
                data: &pixels, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            ))
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            var seen = Set<UInt32>()
            for offset in stride(from: 0, to: pixels.count, by: 4 * 37) {
                seen.insert(UInt32(pixels[offset]) << 16 | UInt32(pixels[offset + 1]) << 8 | UInt32(pixels[offset + 2]))
            }
            return seen.count
        }

        private func storeArtwork() async throws -> Artwork {
            let lookup = try XCTUnwrap(URL(string: "https://itunes.apple.com/search?term=daft+punk+discovery&entity=album&limit=1"))
            let (data, _) = try await URLSession.shared.data(from: lookup)
            struct Search: Decodable {
                struct Result: Decodable { let artworkUrl100: String }
                let results: [Result]
            }
            let thumb = try XCTUnwrap(JSONDecoder().decode(Search.self, from: data).results.first?.artworkUrl100)
            let template = thumb.replacingOccurrences(of: "100x100", with: "{w}x{h}")
            let json = #"{"url":"\#(template)","width":1400,"height":1400}"#
            return try JSONDecoder().decode(Artwork.self, from: Data(json.utf8))
        }

        func testArtworkRendersThroughArtworkImageAtTheRequestedEdge() async throws {
            _ = try requireLive()
            let artwork = try await storeArtwork()
            let template = try XCTUnwrap(artworkTemplate(artwork))
            let url = sized(template, Self.heroEdge)
            let known = try XCTUnwrap(ArtworkRegistry.shared.lookup(url), "templating registers the artwork")
            XCTAssertEqual(known.edge, Self.heroEdge)

            let bytes = try await ArtworkRendering.render(known.artwork, edge: known.edge, url: url)

            let image = try decodedImage(bytes)
            XCTAssertEqual(image.width, Self.heroEdge)
            XCTAssertGreaterThan(try distinctColors(image), 8, "the render captured the cover, not a placeholder fill")
        }

        func testWebArtworkLoadsThroughTheSharedSession() async throws {
            let library = try requireLive()
            let artwork = try await storeArtwork()
            let url = try sized(XCTUnwrap(artworkTemplate(artwork)), Self.heroEdge)

            let bytes = try await library.artwork(url)

            XCTAssertEqual(try decodedImage(bytes).width, Self.heroEdge)
        }

        func testCatalogSearchTakesAppleLimitAndOffset() async throws {
            let library = try requireAuthorized()
            let first = try await catalog("catalog search") { try await library.search("Daft Punk", limit: 25, offset: 0) }
            let second = try await catalog("catalog search") { try await library.search("Daft Punk", limit: 25, offset: 25) }
            XCTAssertFalse(first.songs.isEmpty)
            XCTAssertTrue((first.songs + first.albums + first.artists).allSatisfy { !$0.item.library })
            XCTAssertNotEqual(first.songs.map(\.item.id), second.songs.map(\.item.id), "the offset moves the window")
        }

        func testCatalogArtistChildrenLeadWithTopSongsThenEveryAlbum() async throws {
            let library = try requireAuthorized()
            let found = try await catalog("catalog search") { try await library.search("Daft Punk", limit: 5, offset: 0) }
            let artist = try XCTUnwrap(found.artists.first { $0.title == "Daft Punk" })
            let children = try await catalog("catalog artist") { try await library.children(of: artist.item) }
            let kinds = children.map(\.item.kind)
            let firstAlbum = try XCTUnwrap(kinds.firstIndex(of: .album))
            XCTAssertGreaterThan(firstAlbum, 0, "top songs come first")
            XCTAssertTrue(kinds[firstAlbum...].allSatisfy { $0 == .album })
            XCTAssertTrue(children.contains { $0.title.contains("Discovery") })
        }

        func testCatalogSearchRanksAcrossKindsAndFindsStations() async throws {
            let library = try requireAuthorized()
            let found = try await catalog("catalog search") { try await library.search("Daft Punk", limit: 10, offset: 0) }
            XCTAssertTrue(found.top.contains { $0.item.kind == .artist && $0.title == "Daft Punk" })
            let paged = try await catalog("catalog search") { try await library.search("Daft Punk", limit: 10, offset: 10) }
            XCTAssertTrue(paged.top.isEmpty, "top results belong to the first page only")
            let radio = try await catalog("catalog search") { try await library.search("Pure Pop Radio", limit: 5, offset: 0) }
            XCTAssertFalse(radio.stations.isEmpty)
        }

        func testCatalogAlbumsCarryReleaseDatesAndTheSingleFlag() async throws {
            let library = try requireAuthorized()
            let found = try await catalog("catalog search") { try await library.search("Daft Punk", limit: 5, offset: 0) }
            let artist = try XCTUnwrap(found.artists.first { $0.title == "Daft Punk" })
            let albums = try await catalog("catalog artist") { try await library.children(of: artist.item) }
                .filter { $0.item.kind == .album }
            let homework = try XCTUnwrap(albums.first { $0.title.hasPrefix("Homework") })
            XCTAssertEqual(homework.released?.year, 1997)
            XCTAssertEqual(homework.single, false)
        }

        func testArtistAndSongRelationsNameAlbumArtistAndStation() async throws {
            let library = try requireAuthorized()
            let found = try await catalog("catalog search") { try await library.search("One More Time Daft Punk", limit: 5, offset: 0) }
            let song = try XCTUnwrap(found.songs.first { $0.title.hasPrefix("One More Time") })
            let relations = try await catalog("song relations") { try await library.relations(of: song.item) }
            XCTAssertEqual(relations.album?.kind, .album)
            XCTAssertEqual(relations.artist?.kind, .artist)
            let artist = try XCTUnwrap(relations.artist)
            let artistRelations = try await catalog("artist relations") { try await library.relations(of: artist) }
            XCTAssertEqual(artistRelations.station?.kind, .station)
        }

        func testRecommendationsAndRecents() async throws {
            let library = try requireAuthorized()
            let rails = try await catalog("recommendations") { try await library.recommendations() }
            XCTAssertFalse(rails.isEmpty)
            _ = try await catalog("recents") { try await library.recentlyPlayed(limit: 10, offset: 0) }
        }

        func testFavoritesRead() async throws {
            let library = try requireAuthorized()
            let found = try await catalog("catalog search") { try await library.search("Daft Punk", limit: 3, offset: 0) }
            let refs = found.songs.map(\.item)
            try XCTSkipIf(refs.isEmpty, "no catalog songs to probe")
            let flags = await library.isFavorite(refs)
            XCTAssertEqual(flags.count, refs.count)
        }

        func testLibraryStaplesCarryLibraryProvenanceAndRealCounts() async throws {
            let library = try requireLibrary()
            for scope in [AmLibraryScope.playlists, .albums, .artists] {
                let count = try await library.count(scope)
                var paged: [AmItem] = []
                while true {
                    let page = try await library.page(scope, limit: 100, offset: UInt32(paged.count))
                    paged += page.items
                    if !page.hasMore || page.items.isEmpty { break }
                }
                XCTAssertEqual(Int(count), paged.count, "the \(scope) count matches paging to the end")
                XCTAssertTrue(paged.allSatisfy(\.item.library), "\(scope) items are library refs")
            }
        }

        func testLibraryPlaylistDrilldownReadsEveryBatch() async throws {
            let library = try requireLibrary()
            let playlists = try await library.page(.playlists, limit: 100, offset: 0).items
            try XCTSkipIf(playlists.isEmpty, "account has no library playlists")
            var longest = 0
            for playlist in playlists {
                let tracks = try await library.children(of: playlist.item)
                XCTAssertTrue(tracks.allSatisfy { $0.item.kind == .song && $0.item.library })
                longest = max(longest, tracks.count)
            }
            XCTAssertGreaterThan(longest, 0, "no library playlist returned any tracks")
            if longest <= 100 {
                print("note: the longest library playlist has \(longest) tracks; add one over 100 to exercise batching")
            }
        }

        func testLibraryAlbumAndArtistDrilldowns() async throws {
            let library = try requireLibrary()
            let albums = try await library.page(.albums, limit: 10, offset: 0).items
            try XCTSkipIf(albums.isEmpty, "account has no library albums")
            let tracks = try await library.children(of: albums[0].item)
            XCTAssertFalse(tracks.isEmpty)
            XCTAssertTrue(tracks.allSatisfy { $0.item.kind == .song && $0.item.library })

            let artists = try await library.page(.artists, limit: 10, offset: 0).items
            try XCTSkipIf(artists.isEmpty, "account has no library artists")
            let owned = try await library.children(of: artists[0].item)
            XCTAssertFalse(owned.isEmpty)
            XCTAssertTrue(owned.allSatisfy { $0.item.kind == .album && $0.item.library })
            let resolved = try await library.resolve(artists[0].item)
            XCTAssertEqual(resolved.title, artists[0].title)
        }

        func testLibraryArtworkFetch() async throws {
            let library = try requireLibrary()
            let items = try await library.page(.albums, limit: 10, offset: 0).items
                + library.page(.playlists, limit: 10, offset: 0).items
            let templates = items.compactMap(\.artworkUrl)
            try XCTSkipIf(templates.isEmpty, "no library items with artwork")
            for template in templates.prefix(4) {
                let bytes = try await library.artwork(sized(template, Self.heroEdge))
                XCTAssertGreaterThan(bytes.count, 500, "suspiciously small artwork for \(template)")
                _ = try decodedImage(bytes)
            }
        }

        func testLibrarySongsDeclareTheirProvenance() async throws {
            let library = try requireLibrary()
            let songs = try await library.page(.songs, limit: 5, offset: 0).items
            try XCTSkipIf(songs.isEmpty, "account has no library songs")
            let song = try await MusicKitItems.song(songs[0].item)
            XCTAssertEqual(playParams(song.playParameters)?.isLibrary, true, "transient routing reads this flag")
        }
    }

#endif
