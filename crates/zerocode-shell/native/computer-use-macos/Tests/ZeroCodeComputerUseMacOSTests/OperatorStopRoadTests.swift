import XCTest
@testable import ZeroCodeComputerUseMacOS
@testable import ZeroCodeComputerUseMacOSCore

/// One request line on the helper's wire.
private func stopRoadLine(_ method: String, _ params: [String: Any] = [:]) throws -> Data {
    try JSONSerialization.data(withJSONObject: ["id": 1, "method": method, "params": params])
}

/// The answer to one request line, as the connection's thread answers it.
private func stopRoadAnswer(_ line: Data) -> [String: Any]? {
    guard let request = try? JSONDecoder().decode(Request.self, from: line) else { return nil }
    return handleRequest(provider: Provider(), lock: NSLock(), request: request, expectedToken: nil, authorizedPeer: true) as? [String: Any]
}

/// The person's stop on the helper's own roads (t-66753 A·B): the chord the
/// helper heard stays the person's whatever stops come after it, only the
/// person's resume lifts it, and the window's waiting request hears it at
/// once. Every request goes the way a connection's thread answers it
/// (`handleRequest`); the chord and the window's signal are their own roads'
/// one call (`OperatorGuardHost.stop`) — no key is pressed and no signal sent.
final class OperatorStopRoadTests: XCTestCase {
    private static func ask(_ method: String, _ params: [String: Any] = [:]) throws -> [String: Any] {
        try XCTUnwrap(stopRoadAnswer(try stopRoadLine(method, params)))
    }

    private static func reason() -> String? {
        OperatorGuardHost.status()["reason"] as? String
    }

    /// Whatever a test left standing, the person lifts — the band's resume.
    private static func personLifts() {
        _ = try? ask("resume", ["resetBudget": false, "lifter": "person"])
    }

    /// The person's chord, then the window's signal and an agent's `stop`
    /// (t-66753 A): the stop stays the person's — `status` and the stop's own
    /// answer say `hotkey` — so an agent's resume, naming itself or no one,
    /// lifts nothing; the person's resume lifts it. Counted: how often an
    /// agent's resume lifted the person's stop.
    func test_the_persons_chord_outlasts_later_stops_and_only_their_resume_lifts_it() throws {
        defer { Self.personLifts() }
        OperatorGuardHost.stop(reason: StopReason.hotkey)
        OperatorGuardHost.stop(reason: StopReason.signal)
        let stopped = try XCTUnwrap(try Self.ask("stop")["result"] as? [String: Any])
        XCTAssertEqual(stopped["reason"] as? String, StopReason.hotkey, "the agent's stop answers the person's: \(stopped)")
        XCTAssertEqual(Self.reason(), StopReason.hotkey, "status says the person stopped it after the signal and the agent's stop")
        var liftedByAgent = 0
        for agent: [String: Any] in [["resetBudget": false, "lifter": "agent"], ["resetBudget": false]] {
            _ = try Self.ask("resume", agent)
            if !OperatorGuardHost.isStopped() {
                liftedByAgent += 1
                OperatorGuardHost.stop(reason: StopReason.hotkey)
            }
        }
        XCTAssertEqual(liftedByAgent, 0, "times an agent's resume lifted the person's stop")
        let person = try Self.ask("resume", ["resetBudget": false, "lifter": "person"])
        XCTAssertEqual(person["ok"] as? Bool, true, "\(person)")
        XCTAssertFalse(OperatorGuardHost.isStopped(), "the person's resume lifts it")
    }

    /// A resume that names someone the helper does not know is refused and
    /// lifts nothing (t-66753 A): a word it cannot read is no one's authority.
    func test_a_resume_naming_no_one_the_helper_knows_is_refused() throws {
        defer { Self.personLifts() }
        OperatorGuardHost.stop(reason: StopReason.request)
        let refused = try Self.ask("resume", ["resetBudget": false, "lifter": "anyone"])
        let error = refused["error"] as? [String: Any]
        XCTAssertEqual(error?["code"] as? String, "invalid_argument", "a lifter the helper does not know is refused: \(refused)")
        XCTAssertTrue(OperatorGuardHost.isStopped(), "a word the helper does not know lifts nothing")
    }

    /// An agent's resume still lifts a stop that is not the person's, and the
    /// person's chord over an agent's stop makes the stop theirs (t-66753 A).
    func test_an_agents_resume_lifts_an_agents_stop_and_the_chord_over_it_is_the_persons() throws {
        defer { Self.personLifts() }
        OperatorGuardHost.stop(reason: StopReason.signal)
        _ = try Self.ask("resume", ["resetBudget": false, "lifter": "agent"])
        XCTAssertFalse(OperatorGuardHost.isStopped(), "an agent lifts the stop an agent made")
        OperatorGuardHost.stop(reason: StopReason.request)
        OperatorGuardHost.stop(reason: StopReason.hotkey)
        XCTAssertEqual(Self.reason(), StopReason.hotkey, "the chord over an agent's stop is the person's")
    }

    /// The window's waiting request (t-66753 B): while nothing changes it
    /// waits, and the person's chord answers it at once — not after its wait —
    /// with the stop and the count the window asks after next.
    func test_the_windows_waiting_request_answers_when_the_person_stops() throws {
        // From no stop: a test before this one may have left one standing.
        Self.personLifts()
        defer { Self.personLifts() }
        let before = try XCTUnwrap(try Self.ask("status")["result"] as? [String: Any])
        var watch: [String: Any] = ["ms": 5_000]
        if let revision = before["stopRevision"] {
            watch["after"] = revision
        }
        let request = try stopRoadLine("stopWatch", watch)
        let answer = Box<[String: Any]>()
        let answered = DispatchSemaphore(value: 0)
        Thread.detachNewThread {
            answer.value = stopRoadAnswer(request)
            answered.signal()
        }
        XCTAssertEqual(answered.wait(timeout: .now() + 0.2), .timedOut, "the waiting request waits while nothing changes: \(answer.value ?? [:])")
        let pressed = Date()
        OperatorGuardHost.stop(reason: StopReason.hotkey)
        XCTAssertEqual(answered.wait(timeout: .now() + 5), .success, "the chord answers the waiting request")
        let heardAfter = Date().timeIntervalSince(pressed)
        let result = try XCTUnwrap(answer.value?["result"] as? [String: Any], "the helper answers the window's waiting request: \(answer.value ?? [:])")
        XCTAssertEqual(result["reason"] as? String, StopReason.hotkey)
        XCTAssertNotEqual(result["stopRevision"] as? Int, before["stopRevision"] as? Int, "the count the window asks after next moved")
        XCTAssertLessThan(heardAfter, 1, "the chord ends the wait, not its 5 s")
    }

    /// A waiting request with nothing to hear answers after its wait, with the
    /// stop as it stands (t-66753 B): the window asks again.
    func test_a_waiting_request_answers_after_its_wait_when_nothing_changes() throws {
        Self.personLifts()
        defer { Self.personLifts() }
        let before = try XCTUnwrap(try Self.ask("status")["result"] as? [String: Any])
        let revision = try XCTUnwrap(before["stopRevision"], "status names the count a waiting request asks after: \(before)")
        let began = Date()
        let answered = try Self.ask("stopWatch", ["after": revision, "ms": 50])
        let waited = Date().timeIntervalSince(began)
        let result = try XCTUnwrap(answered["result"] as? [String: Any], "\(answered)")
        XCTAssertEqual(result["stopped"] as? Bool, false)
        XCTAssertGreaterThanOrEqual(waited, 0.05, "it waited its 50 ms")
        XCTAssertLessThan(waited, 2, "and no longer")
    }

    /// The host's resume for an agent while the hand holds a release the
    /// platform refused (t-68324 ②): refused as the person's turn, whether
    /// or not the ledger has a stop — nothing of either is lifted — and the
    /// band's resume lifts both. Counted: how often an agent's resume cleared
    /// the unconfirmed release.
    func test_an_agents_resume_over_an_unconfirmed_release_is_refused_and_lifts_nothing() throws {
        defer { Self.personLifts() }
        Self.personLifts()
        let hand = try OperatorGuardTests.handWithARefusedRelease()
        var clearedByAgent = 0
        for stopFirst in [false, true] {
            if stopFirst { OperatorGuardHost.stop(reason: StopReason.request) }
            let revision = OperatorGuardHost.status()["stopRevision"] as? Int
            XCTAssertThrowsError(try OperatorGuardHost.resume(resetBudget: false, by: .agent, hand: hand)) { error in
                let coded = error as? ProviderError
                XCTAssertEqual(coded?.code, "stopped", "\(error)")
                XCTAssertTrue(coded?.message.contains("only they lift it") == true, "the answer says it is the person's turn: \(error)")
            }
            if hand.snapshot.unconfirmedReleases == 0 { clearedByAgent += 1 }
            XCTAssertEqual(OperatorGuardHost.status()["stopRevision"] as? Int, revision, "the refused resume moved no stop")
        }
        XCTAssertEqual(clearedByAgent, 0, "times an agent's resume cleared the unconfirmed release")
        XCTAssertTrue(OperatorGuardHost.isStopped(), "the stop an agent's refused resume came over stands")
        try OperatorGuardHost.resume(resetBudget: false, by: .person, hand: hand)
        XCTAssertFalse(OperatorGuardHost.isStopped())
        XCTAssertEqual(hand.snapshot.unconfirmedReleases, 0, "the band's resume clears it")
    }
}
