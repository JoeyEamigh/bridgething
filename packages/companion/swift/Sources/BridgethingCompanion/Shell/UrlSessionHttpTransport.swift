import BridgethingCompanionCore
import Foundation

#if canImport(FoundationNetworking)
    import FoundationNetworking
#endif

public final class UrlSessionHttpTransport: NSObject, HttpTransport, @unchecked Sendable {
    private let session: URLSession
    private let downloadRouter: DownloadRouter
    private let downloadSession: URLSession

    override public init() {
        let cfg = URLSessionConfiguration.default
        cfg.timeoutIntervalForRequest = 60
        cfg.timeoutIntervalForResource = 120
        #if canImport(Darwin)
            cfg.networkServiceType = .responsiveData
            cfg.shouldUseExtendedBackgroundIdleMode = true
        #endif
        session = URLSession(configuration: cfg)

        let dlCfg = URLSessionConfiguration.default
        dlCfg.timeoutIntervalForRequest = 60
        dlCfg.timeoutIntervalForResource = TimeInterval(Int32.max)
        #if canImport(Darwin)
            dlCfg.shouldUseExtendedBackgroundIdleMode = true
        #endif
        let router = DownloadRouter()
        downloadRouter = router
        downloadSession = URLSession(configuration: dlCfg, delegate: router, delegateQueue: nil)
        super.init()
    }

    public func execute(request: HttpRequest, sink: HttpSink) {
        guard let req = Self.urlRequest(request) else {
            sink.fail(reason: "invalid url: \(request.url)")
            return
        }
        let task = session.dataTask(with: req) { data, response, error in
            if let error {
                sink.fail(reason: error.localizedDescription)
                return
            }
            guard let http = response as? HTTPURLResponse else {
                sink.fail(reason: "non-http response")
                return
            }
            sink.complete(response: HttpResponse(
                status: UInt16(clamping: http.statusCode),
                headers: Self.headers(http),
                body: data ?? Data()
            ))
        }
        task.resume()
    }

    public func download(request: HttpRequest, sink: HttpDownloadSink) {
        guard let req = Self.urlRequest(request) else {
            sink.onFailed(reason: "invalid url: \(request.url)")
            return
        }
        let task = downloadSession.dataTask(with: req)
        downloadRouter.register(task, sink: sink)
        task.resume()
    }

    static func urlRequest(_ request: HttpRequest) -> URLRequest? {
        guard let url = URL(string: request.url) else { return nil }
        var req = URLRequest(url: url)
        req.httpMethod = switch request.method {
        case .get: "GET"
        case .head: "HEAD"
        case .post: "POST"
        case .put: "PUT"
        case .patch: "PATCH"
        case .delete: "DELETE"
        case .options: "OPTIONS"
        case let .other(verb): verb
        }
        for header in request.headers {
            req.setValue(header.value, forHTTPHeaderField: header.name)
        }
        if !request.body.isEmpty {
            req.httpBody = request.body
        }
        if request.timeoutMs > 0 {
            req.timeoutInterval = TimeInterval(request.timeoutMs) / 1000.0
        }
        return req
    }

    static func headers(_ http: HTTPURLResponse) -> [HttpHeader] {
        http.allHeaderFields.compactMap { key, value -> HttpHeader? in
            guard let name = key as? String else { return nil }
            return HttpHeader(name: name, value: String(describing: value))
        }
    }
}

private final class DownloadRouter: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private var sinks: [Int: HttpDownloadSink] = [:]
    private var responded: Set<Int> = []

    func register(_ task: URLSessionTask, sink: HttpDownloadSink) {
        lock.lock()
        sinks[task.taskIdentifier] = sink
        lock.unlock()
    }

    private func sink(for task: URLSessionTask) -> HttpDownloadSink? {
        lock.lock()
        defer { lock.unlock() }
        return sinks[task.taskIdentifier]
    }

    func urlSession(
        _ session: URLSession, dataTask: URLSessionDataTask,
        didReceive response: URLResponse,
        completionHandler: @escaping (URLSession.ResponseDisposition) -> Void
    ) {
        if let sink = sink(for: dataTask), let http = response as? HTTPURLResponse {
            lock.lock()
            let first = responded.insert(dataTask.taskIdentifier).inserted
            lock.unlock()
            if first {
                sink.onResponse(
                    status: UInt16(clamping: http.statusCode),
                    headers: UrlSessionHttpTransport.headers(http),
                    contentLength: http.expectedContentLength >= 0 ? UInt64(http.expectedContentLength) : nil
                )
            }
        }
        completionHandler(.allow)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        sink(for: dataTask)?.onChunk(chunk: data)
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        lock.lock()
        let sink = sinks.removeValue(forKey: task.taskIdentifier)
        let sawResponse = responded.remove(task.taskIdentifier) != nil
        lock.unlock()
        guard let sink else { return }
        if let error {
            sink.onFailed(reason: error.localizedDescription)
        } else if sawResponse {
            sink.onFinished()
        } else {
            sink.onFailed(reason: "request completed with no http response")
        }
    }
}
