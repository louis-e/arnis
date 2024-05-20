from time import time
from cv2 import imwrite
import numpy as np

from .bresenham import bresenham
from .floodFill import floodFill


def processData(data, args):
    print("Parsing data...")
    resDownScaler = 100
    processingStartTime = time()

    greatestElementX = 0
    greatestElementY = 0
    for element in data["elements"]:
        if element["type"] == "node":
            element["lat"] = int(str(element["lat"]).replace(".", ""))
            element["lon"] = int(str(element["lon"]).replace(".", ""))

            if element["lat"] > greatestElementX:
                greatestElementX = element["lat"]
            if element["lon"] > greatestElementY:
                greatestElementY = element["lon"]

    for element in data["elements"]:
        if element["type"] == "node":
            if len(str(element["lat"])) != len(str(greatestElementX)):
                for i in range(
                    0, len(str(greatestElementX)) - len(str(element["lat"]))
                ):
                    element["lat"] *= 10

            if len(str(element["lon"])) != len(str(greatestElementY)):
                for i in range(
                    0, len(str(greatestElementY)) - len(str(element["lon"]))
                ):
                    element["lon"] *= 10

    lowestElementX = greatestElementX
    lowestElementY = greatestElementY
    for element in data["elements"]:
        if element["type"] == "node":
            if element["lat"] < lowestElementX:
                lowestElementX = element["lat"]
            if element["lon"] < lowestElementY:
                lowestElementY = element["lon"]

    nodesDict = {}
    for element in data["elements"]:
        if element["type"] == "node":
            element["lat"] -= lowestElementX
            element["lon"] -= lowestElementY
            nodesDict[element["id"]] = [element["lat"], element["lon"]]

    orig_posDeterminationCoordX = 0
    orig_posDeterminationCoordY = 0
    map_posDeterminationCoordX = 0
    map_posDeterminationCoordY = 0
    maxBuilding = (0, 0)
    minBuilding = (greatestElementX, greatestElementY)
    nodeIndexList = []
    for i, element in enumerate(data["elements"]):
        if element["type"] == "way":
            for j, node in enumerate(element["nodes"]):
                element["nodes"][j] = nodesDict[node]

            if "tags" in element and "building" in element["tags"]:
                if orig_posDeterminationCoordX == 0:
                    orig_posDeterminationCoordX = element["nodes"][0][0]
                    orig_posDeterminationCoordY = element["nodes"][0][1]
                    map_posDeterminationCoordX = round(
                        element["nodes"][0][0] / resDownScaler
                    )
                    map_posDeterminationCoordY = round(
                        element["nodes"][0][1] / resDownScaler
                    )

                for coordinate in element["nodes"]:
                    cordX = round(coordinate[0] / resDownScaler)
                    cordY = round(coordinate[1] / resDownScaler)

                    if cordX > maxBuilding[0]:
                        maxBuilding = (cordX, maxBuilding[1])
                    elif cordX < minBuilding[0]:
                        minBuilding = (cordX, minBuilding[1])

                    if cordY > maxBuilding[1]:
                        maxBuilding = (maxBuilding[0], cordY)
                    elif cordY < minBuilding[1]:
                        minBuilding = (minBuilding[0], cordY)

        elif element["type"] == "node":
            nodeIndexList.append(i)

    for i in reversed(nodeIndexList):
        del data["elements"][i]

    minBuilding = (minBuilding[0] - 50, minBuilding[1] - 50)
    maxBuilding = (maxBuilding[0] + 50, maxBuilding[1] + 50)
    minMaxDistX = maxBuilding[0] - minBuilding[0]
    minMaxDistY = maxBuilding[1] - minBuilding[1]

    for i, element in enumerate(data["elements"]):
        if element["type"] == "way":
            for j, node in enumerate(element["nodes"]):
                subtractedMinX = (
                    round(element["nodes"][j][0] / resDownScaler) - minBuilding[0]
                )
                subtractedMinY = (
                    round(element["nodes"][j][1] / resDownScaler) - minBuilding[1]
                )

                if subtractedMinX > 0 and subtractedMinX <= minMaxDistX:
                    element["nodes"][j][0] = subtractedMinX
                elif subtractedMinX <= 0 and not (
                    element["nodes"][j][0] > 0 and element["nodes"][j][0] <= minMaxDistX
                ):
                    element["nodes"][j][0] = 0
                if subtractedMinY > 0 and subtractedMinY <= minMaxDistY:
                    element["nodes"][j][1] = subtractedMinY
                elif subtractedMinY <= 0 and not (
                    element["nodes"][j][1] > 0 and element["nodes"][j][1] <= minMaxDistY
                ):
                    element["nodes"][j][1] = 0

                if element["nodes"][j][0] >= minMaxDistX:
                    element["nodes"][j][0] = minMaxDistX - 1
                if element["nodes"][j][1] >= minMaxDistY:
                    element["nodes"][j][1] = minMaxDistY - 1

    if args.debug:
        print(f"minMaxDistX: {minMaxDistX}")
        print(f"minMaxDistY: {minMaxDistY}")
        print(f"Greatest element X: {greatestElementX}")
        print(f"Greatest element Y: {greatestElementY}")
        print(f"Lowest element X: {lowestElementX}")
        print(f"Lowest element Y: {lowestElementY}")
        print(
            "Original position determination reference coordinates: "
            + f"{orig_posDeterminationCoordX}, {orig_posDeterminationCoordY}"
        )
        print(
            "Map position determination reference coordinates: "
            + f"{map_posDeterminationCoordX}, {map_posDeterminationCoordY}"
        )
        with open("arnis-debug-processed_data.json", "w", encoding="utf-8") as f:
            f.write(str(data))

    img = np.zeros(
        (
            minMaxDistY,
            minMaxDistX,
            1,
        ),
        np.uint8,
    )

    img.fill(0)
    imgLanduse = img.copy()

    print("Processing data...")

    ElementIncr = 0
    ElementsLen = len(data["elements"])
    lastProgressPercentage = 0
    for element in reversed(data["elements"]):
        progressPercentage = round(100 * (ElementIncr + 1) / ElementsLen)
        if (
            progressPercentage % 10 == 0
            and progressPercentage != lastProgressPercentage
        ):
            print(f"Element {ElementIncr + 1}/{ElementsLen} ({progressPercentage}%)")
            lastProgressPercentage = progressPercentage

        if element["type"] == "way" and "tags" in element:
            if "building" in element["tags"]:
                previousElement = (0, 0)
                cornerAddup = (0, 0, 0)
                currentBuilding = np.array([[0, 0]])
                for coordinate in element["nodes"]:
                    buildingHeight = 1

                    if previousElement != (0, 0):
                        if "height" in element["tags"]:
                            if len(element["tags"]["height"]) >= 3:
                                buildingHeight = 9
                            elif len(element["tags"]["height"]) == 1:
                                buildingHeight = 2
                            elif element["tags"]["height"][:1] == "1":
                                buildingHeight = 3
                            elif element["tags"]["height"][:1] == "2":
                                buildingHeight = 6
                            else:
                                buildingHeight = 9

                        if (
                            "building:levels" in element["tags"]
                            and element["tags"]["building:levels"].isnumeric()
                            and int(float(element["tags"]["building:levels"])) <= 8
                            and int(float(element["tags"]["building:levels"])) >= 1
                        ):
                            buildingHeight = str(
                                int(float(element["tags"]["building:levels"])) - 1
                            )

                        for i in bresenham(
                            coordinate[0],
                            coordinate[1],
                            previousElement[0],
                            previousElement[1],
                        ):
                            if not (
                                str(img[i[1]][i[0]][0])[:1] == "6"
                                and img[i[1]][i[0]][0] > int("6" + str(buildingHeight))
                            ):
                                img[i[1]][i[0]] = int("6" + str(buildingHeight))

                        currentBuilding = np.append(
                            currentBuilding, [[coordinate[0], coordinate[1]]], axis=0
                        )

                        if not (
                            str(img[coordinate[1]][coordinate[0]][0])[:1] == "5"
                            and img[coordinate[1]][coordinate[0]][0]
                            > int("5" + str(buildingHeight))
                        ):
                            img[coordinate[1]][coordinate[0]] = int(
                                "5" + str(buildingHeight)
                            )

                        if not (
                            str(img[previousElement[1]][previousElement[0]][0])[:1]
                            == "5"
                            and img[previousElement[1]][previousElement[0]][0]
                            > int("5" + str(buildingHeight))
                        ):
                            img[previousElement[1]][previousElement[0]] = int(
                                "5" + str(buildingHeight)
                            )

                        cornerAddup = (
                            cornerAddup[0] + coordinate[0],
                            cornerAddup[1] + coordinate[1],
                            cornerAddup[2] + 1,
                        )
                    previousElement = (coordinate[0], coordinate[1])

                if cornerAddup != (0, 0, 0):
                    img = floodFill(
                        img,
                        round(cornerAddup[1] / cornerAddup[2]),
                        round(cornerAddup[0] / cornerAddup[2]),
                        int("7" + str(buildingHeight)),
                        currentBuilding,
                        minMaxDistX,
                        minMaxDistY,
                        elementType="building",
                    )

            elif "highway" in element["tags"]:
                previousElement = (0, 0)
                for coordinate in element["nodes"]:
                    highwayType = 10
                    if (
                        previousElement != (0, 0)
                        and element["tags"]["highway"] != "corridor"
                        and previousElement != (0, 0)
                        and element["tags"]["highway"] != "steps"
                        and element["tags"]["highway"] != "bridge"
                    ):
                        blockRange = 2
                        highwayType = 10

                        if (
                            element["tags"]["highway"] == "path"
                            or element["tags"]["highway"] == "footway"
                        ):
                            blockRange = 1
                            highwayType = 11
                        elif element["tags"]["highway"] == "motorway":
                            blockRange = 4
                        elif element["tags"]["highway"] == "track":
                            blockRange = 1
                            highwayType = 12
                        elif (
                            "lanes" in element["tags"]
                            and element["tags"]["lanes"] != "1"
                            and element["tags"]["lanes"] != "2"
                        ):
                            blockRange = 4

                        for i in bresenham(
                            coordinate[0],
                            coordinate[1],
                            previousElement[0],
                            previousElement[1],
                        ):
                            for x in range(i[0] - blockRange, i[0] + blockRange + 1):
                                for y in range(
                                    i[1] - blockRange, i[1] + blockRange + 1
                                ):
                                    if (
                                        x < minMaxDistX
                                        and y < minMaxDistY
                                        and img[y][x] == 0
                                    ):
                                        img[y][x] = highwayType
                    previousElement = (coordinate[0], coordinate[1])

            elif "landuse" in element["tags"]:
                previousElement = (0, 0)
                cornerAddup = (0, 0, 0)
                currentLanduse = np.array([[0, 0]])
                for coordinate in element["nodes"]:
                    landuseType = 39
                    if (
                        previousElement != (0, 0)
                        and element["tags"]["landuse"] != "industrial"
                        and element["tags"]["landuse"] != "residential"
                    ):
                        if (
                            element["tags"]["landuse"] == "greenfield"
                            or element["tags"]["landuse"] == "meadow"
                            or element["tags"]["landuse"] == "grass"
                        ):
                            landuseType = 30
                        elif element["tags"]["landuse"] == "farmland":
                            landuseType = 31
                        elif element["tags"]["landuse"] == "forest":
                            landuseType = 32
                        elif element["tags"]["landuse"] == "cemetery":
                            landuseType = 33
                        elif element["tags"]["landuse"] == "beach":
                            landuseType = 34

                        for i in bresenham(
                            coordinate[0],
                            coordinate[1],
                            previousElement[0],
                            previousElement[1],
                        ):
                            if imgLanduse[i[1]][i[0]] == 0:
                                imgLanduse[i[1]][i[0]] = landuseType

                        currentLanduse = np.append(
                            currentLanduse, [[coordinate[0], coordinate[1]]], axis=0
                        )
                        cornerAddup = (
                            cornerAddup[0] + coordinate[0],
                            cornerAddup[1] + coordinate[1],
                            cornerAddup[2] + 1,
                        )
                    previousElement = (coordinate[0], coordinate[1])

                if cornerAddup != (0, 0, 0):
                    imgLanduse = floodFill(
                        imgLanduse,
                        round(cornerAddup[1] / cornerAddup[2]),
                        round(cornerAddup[0] / cornerAddup[2]),
                        landuseType,
                        currentLanduse,
                        minMaxDistX,
                        minMaxDistY,
                    )

            elif "natural" in element["tags"]:
                previousElement = (0, 0)
                cornerAddup = (0, 0, 0)
                currentNatural = np.array([[0, 0]])
                for coordinate in element["nodes"]:
                    naturalType = 39
                    if previousElement != (0, 0):
                        if (
                            element["tags"]["natural"] == "scrub"
                            or element["tags"]["natural"] == "grassland"
                        ):
                            naturalType = 30
                        elif (
                            element["tags"]["natural"] == "beach"
                            or element["tags"]["natural"] == "sand"
                        ):
                            naturalType = 34
                        elif (
                            element["tags"]["natural"] == "wood"
                            or element["tags"]["natural"] == "tree_row"
                        ):
                            naturalType = 32
                        elif element["tags"]["natural"] == "wetland":
                            naturalType = 35
                        elif element["tags"]["natural"] == "water":
                            naturalType = 38

                        for i in bresenham(
                            coordinate[0],
                            coordinate[1],
                            previousElement[0],
                            previousElement[1],
                        ):
                            if imgLanduse[i[1]][i[0]] == 0:
                                imgLanduse[i[1]][i[0]] = naturalType

                        currentNatural = np.append(
                            currentNatural, [[coordinate[0], coordinate[1]]], axis=0
                        )
                        cornerAddup = (
                            cornerAddup[0] + coordinate[0],
                            cornerAddup[1] + coordinate[1],
                            cornerAddup[2] + 1,
                        )
                    previousElement = (coordinate[0], coordinate[1])

                if cornerAddup != (0, 0, 0):
                    if naturalType != 32:
                        imgLanduse = floodFill(
                            imgLanduse,
                            round(cornerAddup[1] / cornerAddup[2]),
                            round(cornerAddup[0] / cornerAddup[2]),
                            naturalType,
                            currentNatural,
                            minMaxDistX,
                            minMaxDistY,
                        )
                    else:
                        imgLanduse = floodFill(
                            imgLanduse,
                            round(cornerAddup[1] / cornerAddup[2]),
                            round(cornerAddup[0] / cornerAddup[2]),
                            naturalType,
                            currentNatural,
                            minMaxDistX,
                            minMaxDistY,
                            elementType="tree_row",
                        )

            elif "leisure" in element["tags"]:
                previousElement = (0, 0)
                cornerAddup = (0, 0, 0)
                currentLeisure = np.array([[0, 0]])
                for coordinate in element["nodes"]:
                    leisureType = 39
                    if (
                        previousElement != (0, 0)
                        and element["tags"]["leisure"] != "marina"
                    ):
                        if (
                            element["tags"]["leisure"] == "park"
                            or element["tags"]["leisure"] == "playground"
                            or element["tags"]["leisure"] == "garden"
                        ):
                            leisureType = 30
                        elif element["tags"]["leisure"] == "pitch":
                            leisureType = 36
                        elif element["tags"]["leisure"] == "swimming_pool":
                            leisureType = 37

                        for i in bresenham(
                            coordinate[0],
                            coordinate[1],
                            previousElement[0],
                            previousElement[1],
                        ):
                            if imgLanduse[i[1]][i[0]] == 0:
                                imgLanduse[i[1]][i[0]] = leisureType

                        currentLeisure = np.append(
                            currentLeisure, [[coordinate[0], coordinate[1]]], axis=0
                        )
                        cornerAddup = (
                            cornerAddup[0] + coordinate[0],
                            cornerAddup[1] + coordinate[1],
                            cornerAddup[2] + 1,
                        )
                    previousElement = (coordinate[0], coordinate[1])

                if cornerAddup != (0, 0, 0):
                    imgLanduse = floodFill(
                        imgLanduse,
                        round(cornerAddup[1] / cornerAddup[2]),
                        round(cornerAddup[0] / cornerAddup[2]),
                        leisureType,
                        currentLeisure,
                        minMaxDistX,
                        minMaxDistY,
                    )

            elif "waterway" in element["tags"]:
                previousElement = (0, 0)
                for coordinate in element["nodes"]:
                    if previousElement != (0, 0) and not (
                        "layer" in element["tags"]
                        and (
                            element["tags"]["layer"] == "-1"
                            or element["tags"]["layer"] == "-2"
                            or element["tags"]["layer"] != "-3"
                        )
                    ):
                        waterwayWidth = 4
                        if "width" in element["tags"]:
                            try:
                                waterwayWidth = int(element["tags"]["width"])
                            except Exception:
                                waterwayWidth = int(float(element["tags"]["width"]))

                        for i in bresenham(
                            coordinate[0],
                            coordinate[1],
                            previousElement[0],
                            previousElement[1],
                        ):
                            for x in range(
                                round(i[0] - waterwayWidth / 2),
                                round(i[0] + waterwayWidth + 1 / 2),
                            ):
                                for y in range(
                                    round(i[1] - waterwayWidth / 2),
                                    round(i[1] + waterwayWidth + 1 / 2),
                                ):
                                    if (
                                        x < minMaxDistX
                                        and y < minMaxDistY
                                        and img[y][x] != 13
                                    ):
                                        img[y][x] = 38
                    previousElement = (coordinate[0], coordinate[1])

            elif "amenity" in element["tags"]:
                previousElement = (0, 0)
                cornerAddup = (0, 0, 0)
                currentAmenity = np.array([[0, 0]])
                amenityType = 20
                for coordinate in element["nodes"]:
                    if previousElement != (0, 0) and (
                        element["tags"]["amenity"] == "parking"
                        or element["tags"]["amenity"] == "fountain"
                    ):
                        if element["tags"]["amenity"] == "parking":
                            amenityType = 20
                        elif element["tags"]["amenity"] == "fountain":
                            amenityType = 21

                        for i in bresenham(
                            coordinate[0],
                            coordinate[1],
                            previousElement[0],
                            previousElement[1],
                        ):
                            if imgLanduse[i[1]][i[0]] == 0:
                                imgLanduse[i[1]][i[0]] = amenityType

                        currentAmenity = np.append(
                            currentAmenity, [[coordinate[0], coordinate[1]]], axis=0
                        )
                        cornerAddup = (
                            cornerAddup[0] + coordinate[0],
                            cornerAddup[1] + coordinate[1],
                            cornerAddup[2] + 1,
                        )
                    previousElement = (coordinate[0], coordinate[1])

                if amenityType == 21:
                    amenityType = 37
                if cornerAddup != (0, 0, 0):
                    imgLanduse = floodFill(
                        imgLanduse,
                        round(cornerAddup[1] / cornerAddup[2]),
                        round(cornerAddup[0] / cornerAddup[2]),
                        amenityType,
                        currentAmenity,
                        minMaxDistX,
                        minMaxDistY,
                    )

            elif "bridge" in element["tags"]:
                previousElement = (0, 0)
                for coordinate in element["nodes"]:
                    if previousElement != (0, 0):
                        for i in bresenham(
                            coordinate[0],
                            coordinate[1],
                            previousElement[0],
                            previousElement[1],
                        ):
                            img[i[1]][i[0]] = 13
                    previousElement = (coordinate[0], coordinate[1])

            elif "railway" in element["tags"]:
                previousElement = (0, 0)
                for coordinate in element["nodes"]:
                    if (
                        previousElement != (0, 0)
                        and element["tags"]["railway"] != "proposed"
                    ):
                        for i in bresenham(
                            coordinate[0] - 2,
                            coordinate[1] - 2,
                            previousElement[0] - 2,
                            previousElement[1] - 2,
                        ):
                            if i[0] < minMaxDistX and i[1] < minMaxDistY:
                                img[i[1]][i[0]] = 14
                        for i in bresenham(
                            coordinate[0] + 1,
                            coordinate[1] + 1,
                            previousElement[0] + 1,
                            previousElement[1] + 1,
                        ):
                            if i[0] < minMaxDistX and i[1] < minMaxDistY:
                                img[i[1]][i[0]] = 14
                    previousElement = (coordinate[0], coordinate[1])

            elif "barrier" in element["tags"]:
                previousElement = (0, 0)
                for coordinate in element["nodes"]:
                    if previousElement != (0, 0):
                        wallHeight = 1
                        if (
                            "height" in element["tags"]
                            and str(element["tags"]["height"])
                            .replace(".", "")
                            .isnumeric()
                        ):
                            wallHeight = round(int(float(element["tags"]["height"])))
                        if wallHeight > 3:
                            wallHeight = 2

                        for i in bresenham(
                            coordinate[0],
                            coordinate[1],
                            previousElement[0],
                            previousElement[1],
                        ):
                            if (
                                str(img[i[1]][i[0]][0])[:1] != 5
                                and str(img[i[1]][i[0]][0])[:1] != 6
                                and str(img[i[1]][i[0]][0])[:1] != 7
                            ):
                                img[i[1]][i[0]] = int("2" + str((wallHeight + 1)))
                    previousElement = (coordinate[0], coordinate[1])

            ElementIncr += 1

    print("Calculating layers...")
    total_pixels = img.shape[0] * img.shape[1]
    processed_pixels = 0

    for x in range(0, img.shape[0]):
        for y in range(0, img.shape[1]):
            if imgLanduse[x][y] != 0 and img[x][y] == 0:
                img[x][y] = imgLanduse[x][y]
            processed_pixels += 1
            percentage = (processed_pixels / total_pixels) * 100
            print(f"Progress: {percentage:.2f}% completed", end='\r')
    print("Progress: 100.00% completed")

    print(
        f"Processing finished in {(time() - processingStartTime):.2f} seconds"
        + f"({((time() - processingStartTime) / 60):.2f} minutes)"
    )
    if args.debug:
        imwrite("arnis-debug-map.png", img)
    return np.flip(img, axis=1)
