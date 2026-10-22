import Foundation
import Network
import Security

/// A fresh, bounded enrollment connection per request. No hello, subscriptions,
/// local identity or work RPC is sent on this server-auth-only TLS channel.
enum CrewdEnrollmentTransport {
    static func call(_ method:String,_ params:Data,link:CrewdEnrollmentLink) async throws -> Data {
        guard ["enrollment.begin","enrollment.commit","enrollment.lookup"].contains(method),params.count<=200000 else{throw CrewdRemoteError.invalidExchange}
        let object=try JSONSerialization.jsonObject(with:params)
        var frame=try JSONSerialization.data(withJSONObject:["id":1,"method":method,"params":object]);frame.append(10)
        guard frame.count<=262144 else{throw CrewdRemoteError.invalidExchange}
        let socket=try EnrollmentSocket(link:link,frame:frame)
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
    var deadline:DispatchWorkItem?
    var operationTimeout:DispatchWorkItem?
    init(link:CrewdEnrollmentLink,frame:Data)throws {
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
                guard !self.cancelled else{continuation.resume(throwing:CrewdRemoteError.invalidExchange);return}
                self.continuation=continuation
                let total=DispatchWorkItem{self.finish(.failure(CrewdRemoteError.invalidExchange))};self.deadline=total
                self.queue.asyncAfter(deadline:.now()+15,execute:total)
                self.timeout()
                self.connection.stateUpdateHandler={state in
                    switch state {
                    case .ready:
                        guard let metadata=self.connection.metadata(definition:NWProtocolTLS.definition) as? NWProtocolTLS.Metadata,
                              let negotiated=sec_protocol_metadata_get_negotiated_protocol(metadata.securityProtocolMetadata),String(cString:negotiated)=="pcw-enroll/1" else{self.finish(.failure(CrewdRemoteError.wrongIdentity));return}
                        self.timeout()
                        self.connection.send(content:self.frame,completion:.contentProcessed{error in
                            guard error==nil else{self.finish(.failure(CrewdRemoteError.invalidExchange));return}
                            self.timeout();self.receive()
                        })
                    case .failed,.cancelled:self.finish(.failure(CrewdRemoteError.invalidExchange))
                    default:break
                    }
                }
                self.connection.start(queue:self.queue)
            }
        }
    }
    func cancel(){queue.async{self.cancelled=true;self.finish(.failure(CancellationError()))}}
    func timeout(){operationTimeout?.cancel();let t=DispatchWorkItem{self.finish(.failure(CrewdRemoteError.invalidExchange))};operationTimeout=t;queue.asyncAfter(deadline:.now()+5,execute:t)}
    func receive(){
        guard !finished else{return}
        connection.receive(minimumIncompleteLength:1,maximumLength:65536){data,_,complete,error in
            guard !self.finished else{return}
            if let data {
                guard data.count<=262144-self.bytes.count else{self.finish(.failure(CrewdRemoteError.invalidExchange));return}
                self.bytes.append(data)
                if let end=self.bytes.firstIndex(of:10){
                    guard self.bytes.index(after:end)==self.bytes.endIndex else{self.finish(.failure(CrewdRemoteError.invalidExchange));return}
                    do {
                        self.finish(.success(try CrewdEnrollmentReply.result(Data(self.bytes[..<end]))))
                    }catch{self.finish(.failure(CrewdRemoteError.invalidExchange))};return
                }
            }
            guard error==nil,!complete else{self.finish(.failure(CrewdRemoteError.invalidExchange));return}
            self.receive()
        }
    }
    func finish(_ result:Result<Data,Error>){
        guard !finished else{return};finished=true;deadline?.cancel();operationTimeout?.cancel()
        connection.stateUpdateHandler=nil;connection.cancel()
        let c=continuation;continuation=nil;c?.resume(with:result)
    }
}
