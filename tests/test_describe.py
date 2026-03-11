from tryke import describe, expect, test

with describe("Math"):
    with describe("addition"):

        @test
        def adds_two_numbers() -> None:
            expect(1 + 1).to_equal(2)

        @test
        async def adds_async() -> None:
            expect(2 + 3).to_equal(5)

    with describe("subtraction"):

        @test
        def subtracts() -> None:
            expect(3 - 1).to_equal(2)


@test
def standalone() -> None:
    expect(True).to_be_truthy()  # noqa: FBT003
