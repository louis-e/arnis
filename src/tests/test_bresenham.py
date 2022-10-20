import collections
import pytest

from src.bresenham import bresenham


TestBresenhamParameters = collections.namedtuple(
    "TestBresenhamParameters", ["x1", "y1", "x2", "y2", "result"]
)


@pytest.mark.parametrize(
    "parameters",
    (
        TestBresenhamParameters(x1=0, y1=0, x2=0, y2=0, result=((0, 0),)),
        TestBresenhamParameters(
            x1=0,
            y1=0,
            x2=5,
            y2=0,
            result=((0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (5, 0)),
        ),
        TestBresenhamParameters(
            x1=0,
            y1=0,
            x2=-5,
            y2=0,
            result=((0, 0), (-1, 0), (-2, 0), (-3, 0), (-4, 0), (-5, 0)),
        ),
        TestBresenhamParameters(
            x1=0,
            y1=0,
            x2=0,
            y2=5,
            result=((0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (0, 5)),
        ),
        TestBresenhamParameters(
            x1=0,
            y1=0,
            x2=0,
            y2=-5,
            result=((0, 0), (0, -1), (0, -2), (0, -3), (0, -4), (0, -5)),
        ),
        TestBresenhamParameters(
            x1=0, y1=0, x2=2, y2=3, result=((0, 0), (1, 1), (1, 2), (2, 3))
        ),
        TestBresenhamParameters(
            x1=0, y1=0, x2=-2, y2=3, result=((0, 0), (-1, 1), (-1, 2), (-2, 3))
        ),
        TestBresenhamParameters(
            x1=0, y1=0, x2=2, y2=-3, result=((0, 0), (1, -1), (1, -2), (2, -3))
        ),
        TestBresenhamParameters(
            x1=0, y1=0, x2=-2, y2=-3, result=((0, 0), (-1, -1), (-1, -2), (-2, -3))
        ),
        TestBresenhamParameters(
            x1=-1,
            y1=-3,
            x2=3,
            y2=3,
            result=((-1, -3), (0, -2), (0, -1), (1, 0), (2, 1), (2, 2), (3, 3)),
        ),
        TestBresenhamParameters(
            x1=0,
            y1=0,
            x2=11,
            y2=1,
            result=(
                (0, 0),
                (1, 0),
                (2, 0),
                (3, 0),
                (4, 0),
                (5, 0),
                (6, 1),
                (7, 1),
                (8, 1),
                (9, 1),
                (10, 1),
                (11, 1),
            ),
        ),
    ),
)
def test_bresenham(parameters: TestBresenhamParameters):
    assert (
        tuple(
            bresenham(
                x1=parameters.x1, y1=parameters.y1, x2=parameters.x2, y2=parameters.y2
            )
        )
        == parameters.result
    )
    assert tuple(
        bresenham(
            x1=parameters.x2, y1=parameters.y2, x2=parameters.x1, y2=parameters.y1
        )
    ) == tuple(reversed(parameters.result))


def test_min_slope_uphill():
    assert tuple(bresenham(x1=0, y1=0, x2=10, y2=1)) == (
        (0, 0),
        (1, 0),
        (2, 0),
        (3, 0),
        (4, 0),
        (5, 1),
        (6, 1),
        (7, 1),
        (8, 1),
        (9, 1),
        (10, 1),
    )


def test_min_slope_downhill():
    assert tuple(bresenham(x1=10, y1=1, x2=0, y2=0)) == (
        (10, 1),
        (9, 1),
        (8, 1),
        (7, 1),
        (6, 1),
        (5, 0),
        (4, 0),
        (3, 0),
        (2, 0),
        (1, 0),
        (0, 0),
    )
