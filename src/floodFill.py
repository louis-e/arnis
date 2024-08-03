from time import time
import numpy as np
import matplotlib.path as mplPath
from polylabel import polylabel


def floodFill(
    img, px, py, newColor, currentBuilding, minMaxDistX, minMaxDistY, elementType="None"
):
    startTimeFloodfill = time()
    currentBuilding = np.delete(currentBuilding, 0, axis=0)
    if len(currentBuilding) <= 2 or not (px < minMaxDistY and py < minMaxDistX):
        return img
    if not (mplPath.Path(currentBuilding).contains_point((py, px))):
        centroid = polylabel([currentBuilding.tolist()], with_distance=True)
        px = round(centroid[0][1])
        py = round(centroid[0][0])
        if not (mplPath.Path(currentBuilding).contains_point((py, px))):
            if mplPath.Path(currentBuilding).contains_point((py - 5, px)):
                py -= 5
            elif mplPath.Path(currentBuilding).contains_point((py + 5, px)):
                py += 5
            elif mplPath.Path(currentBuilding).contains_point((py, px - 5)):
                px -= 5
            elif mplPath.Path(currentBuilding).contains_point((py, px + 5)):
                px += 5
            else:
                return img

    if str(img[px][py][0])[:1] == "5" or str(img[px][py][0])[:1] == "6":
        if mplPath.Path(currentBuilding).contains_point((py - 1, px)):
            py -= 1
        elif mplPath.Path(currentBuilding).contains_point((py + 1, px)):
            py += 1
        elif mplPath.Path(currentBuilding).contains_point((py, px - 1)):
            px -= 1
        elif mplPath.Path(currentBuilding).contains_point((py, px + 1)):
            px += 1
        else:
            return img

    try:
        oldColor = img[px][py][0]
    except Exception:
        return img
    queue = [(px, py)]
    seen = set()
    tot_rows = img.shape[0]
    tot_cols = img.shape[1]
    queueLen = 0
    while queue:
        nxt = []
        for x, y in queue:
            if img[x][y] == newColor:
                continue
            if not (mplPath.Path(currentBuilding).contains_point((y, x))):
                return img
            img[x][y] = newColor
            seen.add((x, y))

            if (
                x
                and (x - 1, y) not in seen
                and (
                    img[x - 1][y] == oldColor
                    or (elementType == "building" and str(img[x - 1][y][0])[:1] == "1")
                )
            ):
                nxt.append((x - 1, y))
            if (
                y
                and (x, y - 1) not in seen
                and (
                    img[x][y - 1] == oldColor
                    or (elementType == "building" and str(img[x][y - 1][0])[:1] == "1")
                )
            ):
                nxt.append((x, y - 1))
            if (
                x < tot_rows - 1
                and (x + 1, y) not in seen
                and (
                    img[x + 1][y] == oldColor
                    or (elementType == "building" and str(img[x + 1][y][0])[:1] == "1")
                )
            ):
                nxt.append((x + 1, y))
            if (
                y < tot_cols - 1
                and (x, y + 1) not in seen
                and (
                    img[x][y + 1] == oldColor
                    or (elementType == "building" and str(img[x][y + 1][0])[:1] == "1")
                )
            ):
                nxt.append((x, y + 1))

            # Timeout ei dirst

        queue = nxt
        if len(nxt) > queueLen:
            queueLen = len(nxt)

    return img
