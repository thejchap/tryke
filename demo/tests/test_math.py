from tryke import describe, expect, test

with describe("arithmetic"):

    @test
    def addition() -> None:
        expect(1 + 1).to_equal(2)

    @test
    def subtraction() -> None:
        expect(10 - 4).to_equal(6)

    @test
    def multiplication() -> None:
        expect(3 * 7).to_equal(21)

    @test
    def division() -> None:
        expect(10 / 2, "halves").to_equal(5)
        expect(10 / 3, "thirds").to_equal(3)
        expect(10 / 5, "fifths").to_equal(2)
