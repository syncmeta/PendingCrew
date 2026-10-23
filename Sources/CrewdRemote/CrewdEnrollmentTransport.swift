import Foundation
import Network
import Security

/// Evidence is local to one socket/request, never inferred from a saved Unknown.
struct CrewdEnrollmentTransportFailure: Error, LocalizedError, Sendable {
    enum Request: String, Sendable { case begin = "enrollment.begin", commit = "enrollment.commit", lookup = "enrollment.lookup" }
    enum Stage: Sendable { case connecting, secureChannelReady, handedToSend, waitingForReply }
    enum Cause: Sendable { case timeout, cancelled, localNetworkDenied, systemDenied, network, dns(Int32), tls(Int32), posix(Int32), unexpectedProtocol, invalidSetup, invalidResponse }
    let request: Request
    let stage: Stage
    let cause: Cause
    var requestWasHandedToSend: Bool { stage == .handedToSend || stage == .waitingForReply }
    var errorDescription: String? {
        let reason: String
        switch cause {
        case .timeout: reason = "连接或响应超时。"
        case .cancelled: reason = "本次连接已取消。"
        case .localNetworkDenied: reason = "系统明确拒绝本地网络访问，请在系统设置的本地网络权限中核对。"
        case .systemDenied: reason = "系统拒绝了本次网络操作。"
        case .network: reason = "网络连接未完成。"
        case .dns(let code): reason = "无法解析电脑地址（DNS \(code)）。"
        case .tls(let code): reason = "安全连接未完成（TLS \(code)）。"
        case .posix(let code): reason = "网络连接未完成（系统错误 \(code)）。"
        case .unexpectedProtocol: reason = "电脑未协商原配对协议。"
        case .invalidSetup: reason = "无法准备本次连接。"
        case .invalidResponse: reason = "未收到可核对的完整响应。"
        }
        if request == .lookup {
            return reason + (requestWasHandedToSend ? "本次查询未取得可确认结果；" : "本次查询未交给发送；") + "原配对结果仍未确认，只查询原请求，不重新配对。"
        }
        return reason + (requestWasHandedToSend
            ? "原请求已交给发送，结果仍未确认。保留原请求，只查询原结果。"
            : "本次请求未交给发送。原请求编号和本设备凭据保留；请读取原配对操作核对当前状态。")
    }
}

struct CrewdEnrollmentTransmission: Sendable {
    let request: CrewdEnrollmentTransportFailure.Request
    private(set) var stage: CrewdEnrollmentTransportFailure.Stage = .connecting
    mutating func secureChannelReady() { stage = .secureChannelReady }
    // Set immediately BEFORE invoking NWConnection.send, including synchronous failure.
    mutating func handedToSend() { stage = .handedToSend }
    mutating func waitingForReply() { stage = .waitingForReply }
    func failure(_ cause: CrewdEnrollmentTransportFailure.Cause) -> CrewdEnrollmentTransportFailure {
        .init(request: request, stage: stage, cause: cause)
    }
}

/// A fresh, bounded enrollment connection per request. No hello, subscriptions,
/// local identity or work RPC is sent on this server-auth-only TLS channel.
enum CrewdEnrollmentTransport {
    static func call(_ method:String,_ params:Data,link:CrewdEnrollmentLink) async throws -> Data {
        guard ["enrollment.begin","enrollment.commit","enrollment.lookup"].contains(method),params.count<=200000 else{throw CrewdRemoteError.invalidExchange}
        let object=try JSONSerialization.jsonObject(with:params)
        var frame=try JSONSerialization.data(withJSONObject:["id":1,"method":method,"params":object]);frame.append(10)
        guard frame.count<=262144 else{throw CrewdRemoteError.invalidExchange}
        guard let request=CrewdEnrollmentTransportFailure.Request(rawValue:method) else{throw CrewdRemoteError.invalidExchange}
        let socket:EnrollmentSocket
        do{socket=try EnrollmentSocket(link:link,frame:frame,request:request)}
        catch{throw CrewdEnrollmentTransportFailure(request:request,stage:.connecting,cause:.invalidSetup)}
        return try await withTaskCancellationHandler(operation:{try await socket.run()},onCancel:{socket.cancel()})
    }
}
private final class EnrollmentSocket:@unchecked Sendable {
    let connection:NWConnection
    let frame:Data
    let queue=DispatchQueue(label:"pendingcrew.enrollment.tls")
    var continuation:CheckedContinuation<Data,Error>?
    var finished=false
    var cancelled=false
    var bytes=Data()
    var transmission:CrewdEnrollmentTransmission
    var deadline:DispatchWorkItem?
    var operationTimeout:DispatchWorkItem?
    init(link:CrewdEnrollmentLink,frame:Data,request:CrewdEnrollmentTransportFailure.Request)throws {
        transmission = .init(request:request)
        self.frame=frame
        let options=NWProtocolTLS.Options()
        sec_protocol_options_set_min_tls_protocol_version(options.securityProtocolOptions,.TLSv13)
        sec_protocol_options_set_max_tls_protocol_version(options.securityProtocolOptions,.TLSv13)
        sec_protocol_options_set_peer_authentication_required(options.securityProtocolOptions,true)
        sec_protocol_options_set_tls_tickets_enabled(options.securityProtocolOptions,false)
        sec_protocol_options_set_tls_resumption_enabled(options.securityProtocolOptions,false)
        sec_protocol_options_add_tls_application_protocol(options.securityProtocolOptions,"pcw-enroll/1")
        let expected=try CrewdRemoteCertificate.decode(link.server.certificateDerBase64)
        let pin=link.server.spkiSha256
        sec_protocol_options_set_verify_block(options.securityProtocolOptions,{_,trust,complete in
            let trust=sec_trust_copy_ref(trust).takeRetainedValue()
            guard let chain=SecTrustCopyCertificateChain(trust) as? [SecCertificate],chain.count==1 else{complete(false);return}
            let der=SecCertificateCopyData(chain[0]) as Data
            do {let (_,actual)=try CrewdRemoteCertificate.validate(der,role:.server);complete(der==expected && actual==pin)}catch{complete(false)}
        },queue)
        guard let port=NWEndpoint.Port(rawValue:link.endpoint.port) else{throw CrewdRemoteError.invalidExchange}
        connection=NWConnection(host:.init(link.endpoint.host),port:port,using:NWParameters(tls:options,tcp:NWProtocolTCP.Options()))
    }
    func run()async throws->Data {
        try await withCheckedThrowingContinuation { continuation in
            queue.async {
                guard !self.cancelled else{continuation.resume(throwing:self.transmission.failure(.cancelled));return}
                self.continuation=continuation
                let total=DispatchWorkItem{self.fail(.timeout)};self.deadline=total
                self.queue.asyncAfter(deadline:.now()+15,execute:total)
                self.timeout()
                self.connection.stateUpdateHandler={state in
                    guard !self.finished else{return}
                    switch state {
                    case .ready:
                        guard let metadata=self.connection.metadata(definition:NWProtocolTLS.definition) as? NWProtocolTLS.Metadata,
                              let negotiated=sec_protocol_metadata_get_negotiated_protocol(metadata.securityProtocolMetadata),String(cString:negotiated)=="pcw-enroll/1" else{self.fail(.unexpectedProtocol);return}
                        self.timeout()
                        self.transmission.secureChannelReady()
                        self.transmission.handedToSend()
                        self.connection.send(content:self.frame,completion:.contentProcessed{error in
                            guard !self.finished else{return}
                            guard error==nil else{self.fail(self.cause(error!));return}
                            self.transmission.waitingForReply();self.timeout();self.receive()
                        })
                    case .failed(let error):self.fail(self.cause(error))
                    case .cancelled:self.fail(.cancelled)
                    case .waiting:
                        if self.connection.currentPath?.unsatisfiedReason == .localNetworkDenied{self.fail(.localNetworkDenied)}
                    default:break
                    }
                }
                self.connection.start(queue:self.queue)
            }
        }
    }
    func cancel(){queue.async{self.cancelled=true;self.fail(.cancelled)}}
    func timeout(){operationTimeout?.cancel();let t=DispatchWorkItem{self.fail(.timeout)};operationTimeout=t;queue.asyncAfter(deadline:.now()+5,execute:t)}
    func receive(){
        guard !finished else{return}
        connection.receive(minimumIncompleteLength:1,maximumLength:65536){data,_,complete,error in
            guard !self.finished else{return}
            if let data {
                guard data.count<=262144-self.bytes.count else{self.fail(.invalidResponse);return}
                self.bytes.append(data)
                if let end=self.bytes.firstIndex(of:10){
                    guard self.bytes.index(after:end)==self.bytes.endIndex else{self.fail(.invalidResponse);return}
                    do {
                        self.finish(.success(try CrewdEnrollmentReply.result(Data(self.bytes[..<end]))))
                    }catch{self.fail(.invalidResponse)};return
                }
            }
            guard error==nil,!complete else{self.fail(.invalidResponse);return}
            self.receive()
        }
    }
    func cause(_ error:NWError)->CrewdEnrollmentTransportFailure.Cause {
        if connection.currentPath?.unsatisfiedReason == .localNetworkDenied{return .localNetworkDenied}
        switch error {
        case .posix(let code):return code == .EACCES || code == .EPERM ? .systemDenied : .posix(code.rawValue)
        case .dns(let code):return .dns(code)
        case .tls(let code):return .tls(code)
        default:return .network
        }
    }
    func fail(_ cause:CrewdEnrollmentTransportFailure.Cause){finish(.failure(transmission.failure(cause)))}
    func finish(_ result:Result<Data,Error>){
        guard !finished else{return};finished=true;deadline?.cancel();operationTimeout?.cancel()
        connection.stateUpdateHandler=nil;connection.cancel()
        let c=continuation;continuation=nil;c?.resume(with:result)
    }
}
