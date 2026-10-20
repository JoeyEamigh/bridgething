import XCTest

@testable import BridgethingCompanion

final class SaturatingConversionTests: XCTestCase {
    func testValuesInRangeTruncateTowardZero() {
        XCTAssertEqual(UInt32(saturating: 0), 0)
        XCTAssertEqual(UInt32(saturating: 800), 800)
        XCTAssertEqual(UInt32(saturating: 3.9), 3)
        XCTAssertEqual(Int32(saturating: -3.9), -3)
        XCTAssertEqual(UInt64(saturating: 9_007_199_254_740_991), 9_007_199_254_740_991)
    }

    func testValuesAboveTheRangeLandOnTheMaximum() {
        XCTAssertEqual(UInt32(saturating: 4_294_967_295), .max)
        XCTAssertEqual(UInt32(saturating: 4_294_967_296), .max)
        XCTAssertEqual(UInt32(saturating: 1e10), .max)
        XCTAssertEqual(UInt32(saturating: 9_007_199_254_740_991), .max)
        XCTAssertEqual(UInt32(saturating: .greatestFiniteMagnitude), .max)
        XCTAssertEqual(UInt32(saturating: .infinity), .max)
        XCTAssertEqual(UInt64(saturating: 18_446_744_073_709_551_616), .max)
        XCTAssertEqual(UInt64(saturating: 1e30), .max)
        XCTAssertEqual(Int64(saturating: 9_223_372_036_854_775_808), .max)
        XCTAssertEqual(Int(saturating: .infinity), .max)
    }

    func testTheLargestDoubleBelowTheMaximumConvertsExactly() {
        XCTAssertEqual(UInt64(saturating: Double(UInt64.max).nextDown), 18_446_744_073_709_549_568)
        XCTAssertEqual(Int64(saturating: Double(Int64.max).nextDown), 9_223_372_036_854_774_784)
        XCTAssertEqual(UInt32(saturating: Double(UInt32.max).nextDown), 4_294_967_294)
    }

    func testValuesBelowTheRangeLandOnTheMinimum() {
        XCTAssertEqual(UInt32(saturating: -1), 0)
        XCTAssertEqual(UInt32(saturating: -0.5), 0)
        XCTAssertEqual(UInt64(saturating: -.infinity), 0)
        XCTAssertEqual(Int64(saturating: -1e30), .min)
        XCTAssertEqual(Int64(saturating: -9_223_372_036_854_775_808), .min)
        XCTAssertEqual(Int8(saturating: -129), .min)
    }

    func testNanIsZero() {
        XCTAssertEqual(UInt32(saturating: .nan), 0)
        XCTAssertEqual(Int64(saturating: .signalingNaN), 0)
    }
}
