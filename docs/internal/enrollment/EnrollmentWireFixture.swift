import Foundation
@main struct EnrollmentWireFixture {
 static func main() throws {
  let object:[String:Any] = ["version":1,"enrollmentId":"11111111-1111-4111-8111-111111111111","server":["authorityId":"22222222-2222-4222-8222-222222222222","dbId":"opaque-db","certificateDerBase64":"Y2VydA==","spkiSha256":String(repeating:"a",count:64)],"endpoint":["host":"154.36.143.228","port":7423],"expiresAt":"2099-01-01T00:00:00Z","computerLabel":"测试主机","token":String(repeating:"A",count:43)]
  let bytes=try JSONSerialization.data(withJSONObject:object,options:[.sortedKeys])
  func uri(_ bytes:Data)->String {"pendingcrew://enroll/v1/"+bytes.base64EncodedString().replacingOccurrences(of:"+",with:"-").replacingOccurrences(of:"/",with:"_").replacingOccurrences(of:"=",with:"")}
  let good=uri(bytes)
  let decoded=try CrewdEnrollmentLink.decode(good)
  precondition(decoded.endpoint.port == 7423)
  let original=String(decoding:bytes,as:UTF8.self)
  let duplicate=original.replacingOccurrences(of:"\"version\":1",with:"\"version\":1,\"vers\\u0069on\":1")
  let cases=[good+"=",good+"?secret=x",uri(Data(duplicate.utf8)),uri(Data(original.replacingOccurrences(of:"\"version\":1",with:"\"version\":2").utf8)),uri(Data(original.replacingOccurrences(of:"\"port\":7423",with:"\"port\":0").utf8)),uri(Data(original.replacingOccurrences(of:"\"computerLabel\":",with:"\"unknown\":true,\"computerLabel\":").utf8))]
  for invalid in cases {do {_ = try CrewdEnrollmentLink.decode(invalid); fatalError("accepted invalid enrollment URI")}catch{}}
  _ = try CrewdEnrollmentReply.result(Data("{\"id\":1,\"result\":{}}".utf8))
  for reply in ["{\"id\":true,\"result\":{}}","{\"id\":2,\"result\":{}}","{\"id\":1,\"result\":{},\"error\":null}","{\"id\":1,\"result\":{},\"result\":{}}"] {do{_ = try CrewdEnrollmentReply.result(Data(reply.utf8));fatalError("accepted malformed reply")}catch{}}
  print("PASS canonical URL, duplicate escaped key, version, endpoint, closed fields")
 }
}
