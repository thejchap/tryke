from tryke import describe, expect, test

with describe("Calculator"):

    @test
    def adds():
        expect(1 + 2, name="sum").to_equal(3)

    @test
    def subtracts():
        expect(5 - 3, name="difference").to_equal(2)

    with describe("edge cases"):

        @test
        def adding_zeros():
            expect(0 + 0, name="zero sum").to_equal(0)

        @test
        def negative_numbers():
            expect(-1 + -1, name="negative sum").to_equal(-2)
