public extension FixedWidthInteger {
    init(saturating value: Double) {
        if value.isNaN {
            self = 0
        } else if value <= Double(Self.min) {
            self = .min
        } else if value >= Double(Self.max) {
            self = .max
        } else {
            self.init(value)
        }
    }
}
