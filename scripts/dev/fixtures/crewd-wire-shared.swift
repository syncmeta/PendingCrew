import Foundation
struct UnsignedProbe:Decodable {let maximum:UInt64;let edge:UInt64;let zero:Int;let missing:String?}
@main struct SharedWireQA {
    static func main()throws {
        let original=Data("{\"maximum\":18446744073709551615,\"edge\":9223372036854775809,\"zero\":0,\"missing\":null,\"unknown\":{\"text\":\"原始🌙\"}}\n".utf8)
        let decoder=CrewdNDJSONDecoder(maximumFrameBytes:1024);var frames:[CrewdValue]=[]
        for byte in original {try decoder.append(Data([byte])){frames.append($0)}}
        try decoder.finish();precondition(frames.count == 1 && frames[0]["unknown"]["text"] == .string("原始🌙"),"byte fragments including split Unicode must deliver exactly one full frame")
        let encoded=try JSONEncoder().encode(frames[0]),probe=try JSONDecoder().decode(UnsignedProbe.self,from:encoded)
        precondition(probe.maximum == UInt64.max && probe.edge == 9_223_372_036_854_775_809 && probe.zero == 0 && probe.missing == nil,"wire roundtrip must preserve full UInt64/unknown zero/null instead of floating rounding")
        precondition(frames[0]["maximum"].int == nil,"unsigned out of Int range must not forge an addressable RPC identifier")
        let compact=Data("{\"id\":1,\"result\":\"ok\"}".utf8)
        let exact=CrewdNDJSONDecoder(maximumFrameBytes:compact.count);var count=0
        try exact.append(compact){_ in count+=1};precondition(count == 0)
        try exact.append(Data([10])){_ in count+=1};try exact.finish();precondition(count == 1)
        let tooSmall=CrewdNDJSONDecoder(maximumFrameBytes:compact.count-1)
        do{try tooSmall.append(compact){_ in preconditionFailure("oversize delivered")};preconditionFailure("oversize accepted")}catch{}
        let eof=CrewdNDJSONDecoder();try eof.append(Data("{\"id\":1".utf8)){_ in preconditionFailure()}
        do{try eof.finish();preconditionFailure("truncated EOF accepted")}catch{}
        let mixed=CrewdNDJSONDecoder();var first:[CrewdValue]=[]
        do{try mixed.append(compact+Data("\nnot-json\n".utf8)){first.append($0)};preconditionFailure("malformed sibling accepted")}catch{}
        precondition(first.count == 1 && first[0]["result"] == .string("ok"),"a later malformed sibling must not swallow an already delivered complete response")
        let multiple=CrewdNDJSONDecoder();var all:[CrewdValue]=[]
        let stream=Data("\n{\"id\":2,\"error\":{\"code\":-32009,\"message\":\"CAS\",\"data\":{\"currentWork\":{\"revision\":\"9007199254740993\"}}}}\n{\"event\":\"work.changed\",\"data\":{\"crewId\":\"a\"}}\n".utf8)
        try multiple.append(stream){all.append($0)};try multiple.finish()
        precondition(all.count == 2 && all[0]["error"]["data"]["currentWork"]["revision"] == .string("9007199254740993") && all[1]["event"] == .string("work.changed"))
        print("PASS: actual shared NDJSON split UTF8/full UInt64 roundtrip/zero-null/exact frame bound/EOF/first response before malformed sibling/structured error and event")
    }
}
