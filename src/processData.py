from math import floor
from time import time
from tqdm import tqdm
import anvil
import matplotlib.path as mplPath
import numpy as np
import random

from .blockDefinitions import *
from .bresenham import bresenham

OFFSET = 1000000000  # Offset to ensure all coordinates are positive
SCALE_FACTOR = 1000000  # Scaling factor

regions = {}

timeout = 2


def floodFillArea(polygon_coords):
    """
    Fast flood fill for polygons with a timeout.

    Parameters:
    - polygon_coords: List of (x, z) tuples defining the polygon.
    - timeout: Maximum time (in seconds) for the flood fill to run.

    Returns:
    - A list of (x, z) tuples representing the filled area within the polygon.
    """
    if len(polygon_coords) < 3:
        return []

    start_time = time()

    # Extract x and z coordinates
    x_coords = [coord[0] for coord in polygon_coords]
    z_coords = [coord[1] for coord in polygon_coords]

    # Create a Path object for the polygon
    poly_path = mplPath.Path(np.array(polygon_coords))

    # Determine the bounding box of the polygon
    min_x, max_x = min(x_coords), max(x_coords)
    min_z, max_z = min(z_coords), max(z_coords)

    # Initialize an empty list to store the filled area
    filled_area = []

    # Use numpy to efficiently check points within the bounding box
    x_range = np.arange(min_x, max_x + 1)
    z_range = np.arange(min_z, max_z + 1)

    # Create a grid of points to test within the bounding box
    xv, zv = np.meshgrid(x_range, z_range, indexing="xy")
    points_to_check = np.vstack((xv.ravel(), zv.ravel())).T

    # Filter points within the polygon
    within_polygon = poly_path.contains_points(points_to_check)

    # Gather the points within the polygon
    filled_area = [
        tuple(points_to_check[i])
        for i in range(len(points_to_check))
        if within_polygon[i]
    ]

    # Check for timeout
    if time() - start_time > timeout:
        return filled_area

    return filled_area


def checkForWater(x, z):
    flooredX = floor(x / 512)
    flooredZ = floor(z / 512)
    identifier = "r." + str(flooredX) + "." + str(flooredZ)

    if identifier not in regions:
        return False

    chunkX = (x // 16) % 32
    chunkZ = (z // 16) % 32

    try:
        chunk = regions[identifier].get_chunk(chunkX, chunkZ)
    except anvil.errors.ChunkNotFound:
        return False

    local_x = x % 16
    local_z = z % 16
    waterlevel_block = chunk.get_block(local_x, 2, local_z)

    return waterlevel_block is not None and waterlevel_block.name() == "minecraft:water"


def setBlock(block, x, y, z, overrideAll=False):
    flooredX = floor(x / 512)
    flooredZ = floor(z / 512)
    identifier = "r." + str(flooredX) + "." + str(flooredZ)

    if identifier not in regions:
        regions[identifier] = anvil.EmptyRegion(0, 0)

    chunkX = (x // 16) % 32
    chunkZ = (z // 16) % 32

    try:
        chunk = regions[identifier].get_chunk(chunkX, chunkZ)
    except anvil.errors.ChunkNotFound:
        chunk = None

    if chunk is None:
        chunk = anvil.EmptyChunk(chunkX, chunkZ)
        regions[identifier].add_chunk(chunk)

    # Get the block within the chunk
    local_x = x % 16
    local_z = z % 16
    y = max(0, min(y, 255))

    existing_block = chunk.get_block(local_x, y, local_z)
    block_below = chunk.get_block(local_x, y - 1, local_z) if y > 1 else grass
    block_below = block_below or grass

    # Check for overwrite rules
    whitelist_existing_block = [
        "minecraft:air",
        "minecraft:dirt",
        "minecraft:grass_block",
    ]
    whitelist_block_below = [
        "minecraft:water",
        "minecraft:light_gray_concrete",
        "minecraft:white_concrete",
        "minecraft:black_concrete",
        "minecraft:gray_concrete",
        "minecraft:dark_oak_door",
    ]
    if overrideAll or (
        (existing_block is None or existing_block.name() in whitelist_existing_block)
        and block_below.name() not in whitelist_block_below
    ):
        chunk.set_block(block, local_x, y, local_z)


def fillBlocks(block, x1, y1, z1, x2, y2, z2, overrideAll=False):
    for x in range(x1, x2 + 1):
        for y in range(y1, y2 + 1):
            for z in range(z1, z2 + 1):
                if not (x == x2 + 1 or y == y2 + 1 or z == z2 + 1):
                    setBlock(block, x, y, z, overrideAll)


def initializeGroundLayer(minMaxDistX, minMaxDistY):
    print("Generating ground layer...")
    chunkSizeX = minMaxDistX // 16
    chunkSizeY = minMaxDistY // 16

    for chunkX in range(chunkSizeX + 1):
        for chunkY in range(chunkSizeY + 1):
            startX = chunkX * 16
            startY = chunkY * 16

            fillBlocks(cobblestone, startX, 0, startY, startX + 15, 0, startY + 15)
            fillBlocks(dirt, startX, 1, startY, startX + 15, 1, startY + 15)
            fillBlocks(grass_block, startX, 2, startY, startX + 15, 2, startY + 15)


def saveRegions(region="all"):
    if region == "all":
        region_keys = list(regions.keys())
        for key in tqdm(
            region_keys,
            desc="Saving minecraft world",
            unit="region",
            bar_format="{l_bar}{bar}| {n_fmt}/{total_fmt}",
        ):
            regions[key].save(mcWorldPath + "/region/" + key + ".mca")
    else:
        regions[region].save(mcWorldPath + "/region/" + region + ".mca")
        print(f"Saved {region}")


priority_order = ["building", "highway", "waterway", "barrier"]


def get_priority(element):
    for i, tag in enumerate(priority_order):
        if tag in element["tags"]:
            return i
    return len(priority_order)


from .tree import createTree  # noqa: E402

# Parse raw data
minMaxDistX = 0
minMaxDistY = 0
startTime = 0
mcWorldPath = ""


def processRawData(data, args):
    global startTime, minMaxDistX, minMaxDistY, mcWorldPath, timeout
    print("Parsing data...")
    resDownScaler = 10
    startTime = time()

    mcWorldPath = args.path
    if mcWorldPath[-1] == "/":
        mcWorldPath = mcWorldPath[:-1]

    timeout = args.timeout

    greatestElementX = -OFFSET
    greatestElementY = -OFFSET
    lowestElementX = OFFSET
    lowestElementY = OFFSET

    included_ways = []

    # Convert all coordinates and determine bounds
    for element in data["elements"]:
        if element["type"] == "node":
            element["lat"] = int((element["lat"] + OFFSET) * SCALE_FACTOR)
            element["lon"] = int((element["lon"] + OFFSET) * SCALE_FACTOR)

            if element["lat"] > greatestElementX:
                greatestElementX = element["lat"]
            if element["lon"] > greatestElementY:
                greatestElementY = element["lon"]
            if element["lat"] < lowestElementX:
                lowestElementX = element["lat"]
            if element["lon"] < lowestElementY:
                lowestElementY = element["lon"]

            if "tags" in element:
                node_whitelist = [
                    "door",
                    "entrance",
                    "natural",
                    "amenity",
                    "barrier",
                    "vending",
                    "highway",
                ]
                if any(tag in element["tags"] for tag in node_whitelist):
                    if (
                        "natural" in element["tags"]
                        and "tree" not in element["tags"]["natural"]
                    ):
                        continue

                    if "amenity" in element["tags"] and (
                        "waste_basket" not in element["tags"]["amenity"]
                        or "bench" not in element["tags"]["amenity"]
                        or "vending_machine" not in element["tags"]["amenity"]
                    ):
                        continue

                    if (
                        "barrier" in element["tags"]
                        and "bollard" not in element["tags"]["barrier"]
                    ):
                        continue

                    if (
                        "highway" in element["tags"]
                        and "street_lamp" not in element["tags"]["highway"]
                    ):
                        continue

                    # Create a temporary way element from the node
                    way = {
                        "type": "way",
                        "id": element["id"],
                        "nodes": [element["id"]],
                        "tags": element["tags"],
                    }
                    included_ways.append(way)

    data["elements"].extend(included_ways)

    if args.debug:
        print(
            f"greatestElementX: {greatestElementX}, greatestElementY: {greatestElementY}"
        )
        print(f"lowestElementX: {lowestElementX}, lowestElementY: {lowestElementY}")

    nodesDict = {}
    for element in data["elements"]:
        if element["type"] == "node":
            nodesDict[element["id"]] = [element["lat"], element["lon"]]

    orig_posDeterminationCoordX = 0
    orig_posDeterminationCoordY = 0
    map_posDeterminationCoordX = 0
    map_posDeterminationCoordY = 0
    maxBuilding = (0, 0)
    minBuilding = (greatestElementX, greatestElementY)
    nodeIndexList = []
    for i, element in enumerate(data["elements"]):
        if element["type"] == "way" and "tags" in element:
            for j, node in enumerate(element["nodes"]):
                element["nodes"][j] = nodesDict[node]

            if "tags" in element and "building" in element["tags"]:
                if orig_posDeterminationCoordX == 0:
                    orig_posDeterminationCoordX = element["nodes"][0][0]
                    orig_posDeterminationCoordY = element["nodes"][0][1]
                    map_posDeterminationCoordX = round(
                        element["nodes"][0][0] / (resDownScaler * 70 / 100)
                    )
                    map_posDeterminationCoordY = round(
                        element["nodes"][0][1] / resDownScaler
                    )

                for coordinate in element["nodes"]:
                    cordX = round(coordinate[0] / (resDownScaler * 70 / 100))
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
        if element["type"] == "way" and "tags" in element:
            for j, node in enumerate(element["nodes"]):
                subtractedMinX = (
                    round(element["nodes"][j][0] / (resDownScaler * 70 / 100))
                    - minBuilding[0]
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

    # Sort elements by priority for layers
    data["elements"].sort(key=get_priority)

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

    return data


# Generate Minecraft world
def generateWorld(data):
    initializeGroundLayer(minMaxDistX, minMaxDistY)

    # Process sorted elements
    groundLevel = 2
    for element in tqdm(
        data["elements"],
        desc="Processing elements",
        unit=" elements",
        total=len(data["elements"]),
        bar_format="{l_bar}{bar}| {n_fmt}/{total_fmt} [{elapsed}<{remaining}, {rate_fmt}]",
    ):
        if element["type"] == "way":
            # Buildings
            if "building" in element["tags"]:
                previousElement = (0, 0)
                cornerAddup = (0, 0, 0)
                currentBuilding = np.array([[0, 0]])

                # Determine the block variation index
                variation_index = random.randint(0, len(building_corner_variations) - 1)

                corner_block = building_corner_variations[variation_index]
                wall_block = building_wall_variations[variation_index]
                floor_block = building_floor_variations[variation_index]

                for coordinate in element["nodes"]:
                    buildingHeight = 4  # Default building height

                    if previousElement != (0, 0):
                        # Check for height attribute
                        if (
                            "height" in element["tags"]
                            and element["tags"]["height"].isdigit()
                        ):
                            buildingHeight = int(element["tags"]["height"])

                        # Check for levels attribute
                        if (
                            "building:levels" in element["tags"]
                            and element["tags"]["building:levels"].isdigit()
                        ):
                            levels = int(element["tags"]["building:levels"])
                            if levels >= 1 and (levels * 3) > buildingHeight:
                                buildingHeight = (
                                    levels * 3
                                )  # Two blocks per level + one for ceiling

                        # Check for special tags
                        if (
                            "building" in element["tags"]
                            and element["tags"]["building"] == "garage"
                        ):
                            buildingHeight = (
                                2  # Garages have a fixed height of 2 blocks
                            )

                        # Calculate walls and corners
                        for coordBresenham in bresenham(
                            coordinate[0],
                            coordinate[1],
                            previousElement[0],
                            previousElement[1],
                        ):
                            for h in range(
                                groundLevel + 1, groundLevel + buildingHeight + 1
                            ):
                                if (
                                    coordBresenham[0] == element["nodes"][0][0]
                                    and coordBresenham[1] == element["nodes"][0][1]
                                ):
                                    setBlock(
                                        corner_block,
                                        coordBresenham[0],
                                        h,
                                        coordBresenham[1],
                                        overrideAll=True,
                                    )
                                else:
                                    setBlock(
                                        wall_block,
                                        coordBresenham[0],
                                        h,
                                        coordBresenham[1],
                                        overrideAll=True,
                                    )
                                setBlock(
                                    cobblestone,
                                    coordBresenham[0],
                                    groundLevel + buildingHeight + 1,
                                    coordBresenham[1],
                                    overrideAll=True,
                                )

                        currentBuilding = np.append(
                            currentBuilding, [[coordinate[0], coordinate[1]]], axis=0
                        )

                        cornerAddup = (
                            cornerAddup[0] + coordinate[0],
                            cornerAddup[1] + coordinate[1],
                            cornerAddup[2] + 1,
                        )
                    previousElement = (coordinate[0], coordinate[1])

                # Floodfill interior with floor variation
                if cornerAddup != (0, 0, 0):
                    polygon_coords = [
                        (coord[0], coord[1]) for coord in element["nodes"]
                    ]
                    floor_area = floodFillArea(polygon_coords)

                    for x, z in floor_area:
                        # Set floor
                        setBlock(floor_block, x, groundLevel, z, overrideAll=True)

                        # Set level ceilings
                        if buildingHeight > 4:
                            for h in range(
                                groundLevel + 4, groundLevel + buildingHeight - 2, 4
                            ):
                                if (x % 6 == 0) and (z % 6 == 0):
                                    setBlock(glowstone, x, h, z)
                                else:
                                    setBlock(floor_block, x, h, z, overrideAll=True)
                        else:
                            if (x % 6 == 0) and (z % 6 == 0):
                                setBlock(glowstone, x, groundLevel + buildingHeight, z)

                        # Set house ceiling
                        setBlock(
                            floor_block,
                            x,
                            groundLevel + buildingHeight + 1,
                            z,
                            overrideAll=True,
                        )

            # Doors
            elif "door" in element["tags"] or "entrance" in element["tags"]:
                if (
                    "level" in element["tags"]
                    and element["tags"]["level"].isdigit()
                    and int(element["tags"]["level"]) != 0
                ):
                    continue

                node_id = element["nodes"][0]
                x = node_id[0]
                z = node_id[1]

                setBlock(gray_concrete, x, groundLevel, z, overrideAll=True)
                setBlock(dark_oak_door_lower, x, groundLevel + 1, z, overrideAll=True)
                setBlock(dark_oak_door_upper, x, groundLevel + 2, z, overrideAll=True)

            # Highways
            elif "highway" in element["tags"]:
                if "street_lamp" in element["tags"]["highway"]:
                    node_id = element["nodes"][0]
                    x = node_id[0]
                    z = node_id[1]
                    setBlock(oak_fence, x, groundLevel + 1, z)
                    setBlock(oak_fence, x, groundLevel + 2, z)
                    setBlock(oak_fence, x, groundLevel + 3, z)
                    setBlock(oak_fence, x, groundLevel + 4, z)
                    setBlock(glowstone, x, groundLevel + 5, z)
                else:
                    previousElement = None
                    blockRange = 2
                    blockType = black_concrete

                    highway_tag = element["tags"]["highway"]
                    lanes = element["tags"].get("lanes")

                    # Determine block range and type based on highway type
                    if highway_tag == "footway":
                        blockType = gray_concrete
                        blockRange = 1
                    elif highway_tag == "path":
                        blockType = light_gray_concrete
                        blockRange = 1
                    elif highway_tag in ["path", "footway"]:
                        if (
                            "footway" in element["tags"]
                            and element["tags"]["footway"] == "crossing"
                        ):
                            blockRange = 1
                            blockType = white_concrete  # For the zebra crossing
                    elif highway_tag == "motorway":
                        blockRange = 4
                    elif highway_tag == "track":
                        blockRange = 1
                    elif lanes and lanes not in ["1", "2"]:
                        blockRange = 4

                    for coordinate in element["nodes"]:
                        if previousElement is not None:
                            if highway_tag == "bridge":
                                for coordBresenham in bresenham(
                                    coordinate[0],
                                    coordinate[1],
                                    previousElement[0],
                                    previousElement[1],
                                ):
                                    for x in range(
                                        coordBresenham[0] - blockRange,
                                        coordBresenham[0] + blockRange + 1,
                                    ):
                                        for y in range(
                                            coordBresenham[1] - blockRange,
                                            coordBresenham[1] + blockRange + 1,
                                        ):
                                            height = (
                                                groundLevel
                                                + 1
                                                + (
                                                    abs(
                                                        coordinate[1]
                                                        - previousElement[1]
                                                    )
                                                    // 16
                                                )
                                            )  # Gradual elevation
                                            setBlock(light_gray_concrete, x, height, y)
                                            setBlock(
                                                cobblestone_wall, x, height + 1, y
                                            )  # Railings
                            elif highway_tag == "steps":
                                for coordBresenham in bresenham(
                                    coordinate[0],
                                    coordinate[1],
                                    previousElement[0],
                                    previousElement[1],
                                ):
                                    for x in range(
                                        coordBresenham[0] - blockRange,
                                        coordBresenham[0] + blockRange + 1,
                                    ):
                                        for y in range(
                                            coordBresenham[1] - blockRange,
                                            coordBresenham[1] + blockRange + 1,
                                        ):
                                            height = groundLevel + (
                                                abs(coordinate[1] - previousElement[1])
                                                // 16
                                            )  # Elevate for steps
                                            setBlock(stone, x, height, y)
                            else:
                                for coordBresenham in bresenham(
                                    coordinate[0],
                                    coordinate[1],
                                    previousElement[0],
                                    previousElement[1],
                                ):
                                    for x in range(
                                        coordBresenham[0] - blockRange,
                                        coordBresenham[0] + blockRange + 1,
                                    ):
                                        for y in range(
                                            coordBresenham[1] - blockRange,
                                            coordBresenham[1] + blockRange + 1,
                                        ):
                                            if (
                                                highway_tag == "footway"
                                                and element["tags"].get("footway")
                                                == "crossing"
                                            ):
                                                is_horizontal = abs(
                                                    coordinate[0] - previousElement[0]
                                                ) >= abs(
                                                    coordinate[1] - previousElement[1]
                                                )
                                                if is_horizontal:
                                                    if coordBresenham[0] % 4 < 2:
                                                        setBlock(
                                                            white_concrete,
                                                            x,
                                                            groundLevel,
                                                            y,
                                                        )
                                                    else:
                                                        setBlock(
                                                            black_concrete,
                                                            x,
                                                            groundLevel,
                                                            y,
                                                        )
                                                else:
                                                    if coordBresenham[1] % 4 < 2:
                                                        setBlock(
                                                            white_concrete,
                                                            x,
                                                            groundLevel,
                                                            y,
                                                        )
                                                    else:
                                                        setBlock(
                                                            black_concrete,
                                                            x,
                                                            groundLevel,
                                                            y,
                                                        )
                                            else:
                                                setBlock(blockType, x, groundLevel, y)
                        previousElement = coordinate

            # Landuse
            elif "landuse" in element["tags"]:
                previousElement = None
                cornerAddup = np.array([0, 0, 0])
                currentLanduse = []

                landuse_tag = element["tags"]["landuse"]
                blockType = {
                    "greenfield": grass_block,
                    "meadow": grass_block,
                    "grass": grass_block,
                    "farmland": farmland,
                    "forest": grass_block,
                    "cemetery": podzol,
                    "beach": sand,
                    "construction": dirt,
                    "traffic_island": stone_block_slab,
                }.get(landuse_tag, grass_block)

                # Process landuse nodes
                for coordinate in element["nodes"]:
                    if previousElement is not None:
                        for coordBresenham in bresenham(
                            coordinate[0],
                            coordinate[1],
                            previousElement[0],
                            previousElement[1],
                        ):
                            setBlock(
                                grass_block,
                                coordBresenham[0],
                                groundLevel,
                                coordBresenham[1],
                            )

                        currentLanduse.append((coordinate[0], coordinate[1]))
                        cornerAddup += np.array([coordinate[0], coordinate[1], 1])

                    previousElement = coordinate

                if len(currentLanduse) > 0:
                    polygon_coords = [
                        (coord[0], coord[1]) for coord in element["nodes"]
                    ]
                    floor_area = floodFillArea(polygon_coords)

                    for x, z in floor_area:
                        setBlock(blockType, x, groundLevel, z)
                        if landuse_tag == "traffic_island":
                            setBlock(blockType, x, groundLevel + 1, z)

                        # Add specific features for different landuse types
                        if landuse_tag == "cemetery":
                            if (x % 3 == 0) and (z % 3 == 0):
                                randomChoice = random.randint(0, 100)
                                if randomChoice < 15:  # 15% chance for a grave
                                    if random.randint(0, 1) == 0:
                                        setBlock(cobblestone, x - 1, groundLevel + 1, z)
                                        setBlock(
                                            stone_brick_slab, x - 1, groundLevel + 2, z
                                        )
                                        setBlock(
                                            stone_brick_slab, x, groundLevel + 1, z
                                        )
                                        setBlock(
                                            stone_brick_slab, x + 1, groundLevel + 1, z
                                        )
                                    else:
                                        setBlock(cobblestone, x, groundLevel + 1, z - 1)
                                        setBlock(
                                            stone_brick_slab, x, groundLevel + 2, z - 1
                                        )
                                        setBlock(
                                            stone_brick_slab, x, groundLevel + 1, z
                                        )
                                        setBlock(
                                            stone_brick_slab, x, groundLevel + 1, z + 1
                                        )
                                elif randomChoice < 30:  # 15% chance for a flower
                                    setBlock(red_flower, x, groundLevel + 1, z)
                                elif randomChoice < 33:  # 3% chance for a tree
                                    createTree(
                                        x, groundLevel + 1, z, random.randint(1, 3)
                                    )

                        elif landuse_tag == "forest":
                            if checkForWater(x, z):
                                continue
                            randomChoice = random.randint(0, 20)
                            randomTree = random.randint(1, 3)
                            randomFlower = random.randint(1, 4)
                            if randomChoice == 20:
                                createTree(x, groundLevel + 1, z, randomTree)
                            elif randomChoice == 2:
                                if randomFlower == 1:
                                    setBlock(red_flower, x, groundLevel + 1, z)
                                elif randomFlower == 2:
                                    setBlock(blue_flower, x, groundLevel + 1, z)
                                elif randomFlower == 3:
                                    setBlock(yellow_flower, x, groundLevel + 1, z)
                                else:
                                    setBlock(white_flower, x, groundLevel + 1, z)
                            elif randomChoice == 0 or randomChoice == 1:
                                setBlock(grass, x, groundLevel + 1, z)

                        elif landuse_tag == "farmland":
                            if checkForWater(x, z):
                                continue
                            if x % 15 == 0 or z % 15 == 0:
                                setBlock(water, x, groundLevel, z, overrideAll=True)
                            else:
                                setBlock(farmland, x, groundLevel, z)
                                if (
                                    random.randint(0, 75) == 0
                                ):  # Rarely place trees, hay bales, or leaves
                                    special_choice = random.randint(1, 10)
                                    if special_choice <= 1:  # 10% chance for a tree
                                        createTree(
                                            x, groundLevel + 1, z, random.randint(1, 3)
                                        )
                                    elif (
                                        special_choice <= 6
                                    ):  # 50% chance for hay bales
                                        setBlock(hay_bale, x, groundLevel + 1, z)
                                        if random.randint(0, 2) == 0:
                                            setBlock(hay_bale, x, groundLevel + 2, z)
                                            setBlock(
                                                hay_bale, x - 1, groundLevel + 1, z
                                            )
                                            setBlock(
                                                hay_bale, x, groundLevel + 1, z - 1
                                            )
                                        else:
                                            setBlock(hay_bale, x, groundLevel + 2, z)
                                            setBlock(
                                                hay_bale, x + 1, groundLevel + 1, z
                                            )
                                            setBlock(
                                                hay_bale, x, groundLevel + 1, z + 1
                                            )
                                    else:  # 40% chance for leaves
                                        setBlock(oak_leaves, x, groundLevel + 1, z)
                                else:  # Otherwise, place crops
                                    crop_choice = random.randint(0, 2)
                                    crops = [wheat, carrots, potatoes]
                                    setBlock(crops[crop_choice], x, groundLevel + 1, z)

                        elif landuse_tag == "construction":
                            randomChoice = random.randint(0, 1500)

                            # Random chance distribution
                            if randomChoice < 6:  # Scaffolding
                                setBlock(scaffolding, x, groundLevel + 1, z)
                                if randomChoice < 2:
                                    setBlock(scaffolding, x, groundLevel + 2, z)
                                    setBlock(scaffolding, x, groundLevel + 3, z)
                                elif randomChoice < 4:
                                    setBlock(scaffolding, x, groundLevel + 2, z)
                                    setBlock(scaffolding, x, groundLevel + 3, z)
                                    setBlock(scaffolding, x, groundLevel + 4, z)
                                    setBlock(scaffolding, x, groundLevel + 1, z + 1)
                                else:
                                    setBlock(scaffolding, x, groundLevel + 2, z)
                                    setBlock(scaffolding, x, groundLevel + 3, z)
                                    setBlock(scaffolding, x, groundLevel + 4, z)
                                    setBlock(scaffolding, x, groundLevel + 5, z)
                                    setBlock(scaffolding, x - 1, groundLevel + 1, z)
                                    setBlock(scaffolding, x + 1, groundLevel + 1, z - 1)
                            elif randomChoice < 20:  # Random blocks
                                construction_items = [
                                    oak_log,
                                    cobblestone,
                                    gravel,
                                    glowstone,
                                    stone,
                                    cobblestone_wall,
                                    black_concrete,
                                    sand,
                                    oak_planks,
                                    dirt,
                                    brick,
                                ]
                                setBlock(
                                    random.choice(construction_items),
                                    x,
                                    groundLevel + 1,
                                    z,
                                )
                            elif randomChoice < 25:  # Dirt pile
                                if randomChoice < 20:
                                    setBlock(dirt, x, groundLevel + 1, z)
                                    setBlock(dirt, x, groundLevel + 2, z)
                                    setBlock(dirt, x + 1, groundLevel + 1, z)
                                    setBlock(dirt, x, groundLevel + 1, z + 1)
                                else:
                                    setBlock(dirt, x, groundLevel + 1, z)
                                    setBlock(dirt, x, groundLevel + 2, z)
                                    setBlock(dirt, x - 1, groundLevel + 1, z)
                                    setBlock(dirt, x, groundLevel + 1, z - 1)
                            elif randomChoice < 140:  # Ground roughness
                                setBlock(air, x, groundLevel, z, overrideAll=True)

                        elif landuse_tag == "grass":
                            if random.randint(1, 7) != 0:
                                if checkForWater(x, z):
                                    continue
                                setBlock(grass, x, groundLevel + 1, z)

                        elif landuse_tag == "meadow":
                            if checkForWater(x, z):
                                continue
                            randomChoice = random.randint(0, 1000)
                            if randomChoice < 5:
                                createTree(x, groundLevel + 1, z, random.randint(1, 3))
                            elif randomChoice < 800:
                                setBlock(grass, x, groundLevel + 1, z)

            # Natural
            elif "natural" in element["tags"]:
                if "tree" in element["tags"]["natural"]:
                    node_id = element["nodes"][0]
                    x = node_id[0]
                    z = node_id[1]
                    createTree(x, groundLevel + 1, z, random.randint(1, 3))
                else:
                    previousElement = (0, 0)
                    cornerAddup = (0, 0, 0)
                    currentNatural = np.array([[0, 0]])
                    blockType = grass_block

                    natural_type_mapping = {
                        "scrub": grass_block,
                        "grassland": grass_block,
                        "beach": sand,
                        "sand": sand,
                        "wood": grass_block,
                        "tree_row": oak_leaves,
                        "wetland": water,
                        "water": water,
                    }

                    blockType = natural_type_mapping.get(
                        element["tags"]["natural"], grass_block
                    )

                    for coordinate in element["nodes"]:
                        if previousElement != (0, 0):
                            for coordBresenham in bresenham(
                                coordinate[0],
                                coordinate[1],
                                previousElement[0],
                                previousElement[1],
                            ):
                                setBlock(
                                    blockType,
                                    coordBresenham[0],
                                    groundLevel,
                                    coordBresenham[1],
                                )

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
                        polygon_coords = [
                            (coord[0], coord[1]) for coord in element["nodes"]
                        ]
                        filled_area = floodFillArea(polygon_coords)

                        for x, z in filled_area:
                            setBlock(blockType, x, groundLevel, z)

                            # Forest element generation
                            if element["tags"]["natural"] in ["wood", "tree_row"]:
                                if checkForWater(x, z):
                                    continue
                                randomChoice = random.randint(0, 25)
                                randomTree = random.randint(1, 3)
                                randomFlower = random.randint(1, 4)
                                if randomChoice == 25:
                                    createTree(x, groundLevel + 1, z, randomTree)
                                elif randomChoice == 2:
                                    if randomFlower == 1:
                                        setBlock(red_flower, x, groundLevel + 1, z)
                                    elif randomFlower == 2:
                                        setBlock(blue_flower, x, groundLevel + 1, z)
                                    elif randomFlower == 3:
                                        setBlock(yellow_flower, x, groundLevel + 1, z)
                                    else:
                                        setBlock(white_flower, x, groundLevel + 1, z)
                                elif randomChoice in [0, 1]:
                                    setBlock(grass, x, groundLevel + 1, z)

            # Amenities
            elif "amenity" in element["tags"]:
                if (
                    "waste_disposal" in element["tags"]["amenity"]
                    or "waste_basket" in element["tags"]["amenity"]
                ):
                    node_id = element["nodes"][0]
                    x = node_id[0]
                    z = node_id[1]
                    setBlock(cauldron, x, groundLevel + 1, z)
                elif "bench" in element["tags"]["amenity"]:
                    node_id = element["nodes"][0]
                    x = node_id[0]
                    z = node_id[1]
                    setBlock(
                        anvil.Block("minecraft", "oak_slab", {"type": "top"}),
                        x,
                        groundLevel + 1,
                        z,
                    )
                    setBlock(oak_log, x + 1, groundLevel + 1, z)
                    setBlock(oak_log, x - 1, groundLevel + 1, z)
                elif "vending" in element["tags"]:
                    node_id = element["nodes"][0]
                    x = node_id[0]
                    z = node_id[1]

                    setBlock(iron_block, x, groundLevel + 1, z)
                    setBlock(iron_block, x, groundLevel + 2, z)
                else:
                    previousElement = None
                    cornerAddup = np.array([0, 0, 0])
                    currentAmenity = []

                    for coordinate in element["nodes"]:
                        if previousElement:
                            if element["tags"]["amenity"] in ["parking", "fountain"]:
                                blockType = (
                                    water
                                    if element["tags"]["amenity"] == "fountain"
                                    else gray_concrete
                                )

                                for coordBresenham in bresenham(
                                    coordinate[0],
                                    coordinate[1],
                                    previousElement[0],
                                    previousElement[1],
                                ):
                                    setBlock(
                                        blockType,
                                        coordBresenham[0],
                                        groundLevel,
                                        coordBresenham[1],
                                    )

                                    if element["tags"]["amenity"] == "fountain":
                                        # Decorative border around the fountain
                                        for dx in [-1, 0, 1]:
                                            for dz in [-1, 0, 1]:
                                                if (dx, dz) != (0, 0):
                                                    setBlock(
                                                        light_gray_concrete,
                                                        coordBresenham[0] + dx,
                                                        groundLevel,
                                                        coordBresenham[1] + dz,
                                                    )

                                currentAmenity.append(coordinate)
                                cornerAddup += np.array(
                                    [coordinate[0], coordinate[1], 1]
                                )
                        previousElement = coordinate

                    if cornerAddup[2] > 0:
                        flood_area = floodFillArea(
                            [(coord[0], coord[1]) for coord in currentAmenity]
                        )

                        for x, z in flood_area:
                            setBlock(blockType, x, groundLevel, z)
                            if (
                                element["tags"]["amenity"] == "parking"
                                and (x + z) % 8 == 0
                                and (x * z) % 32 != 0
                            ):
                                setBlock(
                                    light_gray_concrete,
                                    x,
                                    groundLevel,
                                    z,
                                    overrideAll=True,
                                )

            # Leisure
            elif "leisure" in element["tags"]:
                previousElement = (0, 0)
                cornerAddup = (0, 0, 0)
                currentLeisure = np.array([[0, 0]])
                blockType = grass_block

                leisure_type_mapping = {
                    "park": grass_block,
                    "playground": green_stained_hardened_clay,
                    "garden": grass_block,
                    "pitch": green_stained_hardened_clay,
                    "swimming_pool": water,
                }

                blockType = leisure_type_mapping.get(
                    element["tags"]["leisure"], grass_block
                )

                for coordinate in element["nodes"]:
                    if previousElement != (0, 0):
                        for coordBresenham in bresenham(
                            coordinate[0],
                            coordinate[1],
                            previousElement[0],
                            previousElement[1],
                        ):
                            setBlock(
                                blockType,
                                coordBresenham[0],
                                groundLevel,
                                coordBresenham[1],
                            )

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
                    polygon_coords = [
                        (coord[0], coord[1]) for coord in element["nodes"]
                    ]
                    filled_area = floodFillArea(polygon_coords)

                    for x, z in filled_area:
                        setBlock(blockType, x, groundLevel, z)

                        # Add decorative elements for parks and gardens
                        if element["tags"]["leisure"] in ["park", "garden"]:
                            if checkForWater(x, z):
                                continue
                            randomChoice = random.randint(
                                0, 1000
                            )  # Random chance distribution
                            if randomChoice < 1:  # Benches
                                setBlock(oak_log, x, groundLevel + 1, z)
                                setBlock(oak_log, x + 1, groundLevel + 1, z)
                                setBlock(oak_log, x - 1, groundLevel + 1, z)
                            elif randomChoice < 30:  # Flowers
                                flower_choice = random.choice(
                                    [
                                        red_flower,
                                        yellow_flower,
                                        blue_flower,
                                        white_flower,
                                    ]
                                )
                                setBlock(flower_choice, x, groundLevel + 1, z)
                            elif randomChoice < 70:  # Grass
                                setBlock(grass, x, groundLevel + 1, z)
                            elif randomChoice < 80:  # Tree
                                createTree(x, groundLevel + 1, z, random.randint(1, 3))

            # Waterways
            elif "waterway" in element["tags"]:
                previousElement = (0, 0)
                waterwayWidth = 4

                # Check for custom width in tags
                if "width" in element["tags"]:
                    try:
                        waterwayWidth = int(element["tags"]["width"])
                    except ValueError:
                        waterwayWidth = int(float(element["tags"]["width"]))

                for coordinate in element["nodes"]:
                    if previousElement != (0, 0) and not (
                        "layer" in element["tags"]
                        and element["tags"]["layer"] in ["-1", "-2", "-3"]
                    ):
                        for coordBresenham in bresenham(
                            coordinate[0],
                            coordinate[1],
                            previousElement[0],
                            previousElement[1],
                        ):
                            for x in range(
                                round(coordBresenham[0] - waterwayWidth / 2),
                                round(coordBresenham[0] + waterwayWidth / 2) + 1,
                            ):
                                for z in range(
                                    round(coordBresenham[1] - waterwayWidth / 2),
                                    round(coordBresenham[1] + waterwayWidth / 2) + 1,
                                ):
                                    setBlock(water, x, groundLevel, z, overrideAll=True)

                    previousElement = (coordinate[0], coordinate[1])

            # Bridges
            elif "bridge" in element["tags"]:
                if "layer" in element["tags"]:
                    bridge_height = int(element["tags"]["layer"])
                else:
                    bridge_height = 1  # Default height if not specified

                # Calculate the total length of the bridge
                total_steps = sum(
                    len(
                        list(
                            bresenham(
                                element["nodes"][i][0],
                                element["nodes"][i][1],
                                element["nodes"][i - 1][0],
                                element["nodes"][i - 1][1],
                            )
                        )
                    )
                    for i in range(1, len(element["nodes"]))
                )

                half_steps = (
                    total_steps // 2
                )  # Calculate midpoint for descending after rising

                current_step = 0
                for i, coordinate in enumerate(element["nodes"]):
                    if i > 0:
                        prev_coord = element["nodes"][i - 1]
                        for coordBresenham in bresenham(
                            coordinate[0],
                            coordinate[1],
                            prev_coord[0],
                            prev_coord[1],
                        ):
                            # Calculate the current height of the bridge
                            if current_step <= half_steps:
                                current_height = (
                                    groundLevel + bridge_height + current_step // 5
                                )  # Rise for the first half
                            else:
                                current_height = (
                                    groundLevel
                                    + bridge_height
                                    + (half_steps // 5)
                                    - ((current_step - half_steps) // 5)
                                )  # Descend for the second half

                            setBlock(
                                light_gray_concrete,
                                coordBresenham[0],
                                current_height,
                                coordBresenham[1],
                            )
                            for offsetX, offsetZ in [
                                (-1, -1),
                                (1, -1),
                                (1, 1),
                                (-1, 1),
                            ]:
                                setBlock(
                                    light_gray_concrete,
                                    coordBresenham[0] + offsetX,
                                    current_height,
                                    coordBresenham[1] + offsetZ,
                                )

                            current_step += 1

            # Railways
            elif "railway" in element["tags"]:
                if element["tags"]["railway"] not in [
                    "proposed",
                    "abandoned",
                    "subway",
                ]:
                    if (
                        "subway" in element["tags"]
                        and "yes" in element["tags"]["subway"]
                    ):
                        continue

                    for i, coordinate in enumerate(element["nodes"]):
                        if i > 0:
                            prev_coord = element["nodes"][i - 1]
                            for coordBresenham in bresenham(
                                coordinate[0],
                                coordinate[1],
                                prev_coord[0],
                                prev_coord[1],
                            ):
                                # Determine direction and set rail shape accordingly
                                dx = coordBresenham[0] - prev_coord[0]
                                dz = coordBresenham[1] - prev_coord[1]

                                if dx == 0 and dz > 0:
                                    shape = "north_south"
                                elif dx == 0 and dz < 0:
                                    shape = "north_south"
                                elif dz == 0 and dx > 0:
                                    shape = "east_west"
                                elif dz == 0 and dx < 0:
                                    shape = "east_west"
                                elif dx > 0 and dz > 0:
                                    shape = "south_east"
                                elif dx < 0 and dz < 0:
                                    shape = "north_west"
                                elif dx > 0 and dz < 0:
                                    shape = "north_east"
                                elif dx < 0 and dz > 0:
                                    shape = "south_west"

                                rail_block = anvil.Block(
                                    "minecraft", "rail", {"shape": shape}
                                )

                                setBlock(
                                    iron_block,
                                    coordBresenham[0],
                                    groundLevel,
                                    coordBresenham[1],
                                )
                                setBlock(
                                    rail_block,
                                    coordBresenham[0],
                                    groundLevel + 1,
                                    coordBresenham[1],
                                )

                                if coordBresenham[0] % 4 == 0:
                                    setBlock(
                                        oak_log,
                                        coordBresenham[0],
                                        groundLevel,
                                        coordBresenham[1],
                                    )

            # Barriers
            elif "barrier" in element["tags"]:
                if "bollard" in element["tags"]["barrier"]:
                    node_id = element["nodes"][0]
                    x = node_id[0]
                    z = node_id[1]
                    setBlock(cobblestone_wall, x, groundLevel + 1, z, overrideAll=True)
                else:
                    if (
                        "height" in element["tags"]
                        and element["tags"]["height"].replace(".", "").isdigit()
                    ):
                        wallHeight = min(
                            3, round(int(float(element["tags"]["height"])))
                        )
                    else:
                        wallHeight = 2

                    for i, coordinate in enumerate(element["nodes"]):
                        if i > 0:
                            prev_coord = element["nodes"][i - 1]
                            for coordBresenham in bresenham(
                                coordinate[0],
                                coordinate[1],
                                prev_coord[0],
                                prev_coord[1],
                            ):
                                # Build the barrier wall to the specified height
                                for y in range(
                                    groundLevel + 1, groundLevel + wallHeight + 1
                                ):
                                    setBlock(
                                        cobblestone_wall,
                                        coordBresenham[0],
                                        y,
                                        coordBresenham[1],
                                        overrideAll=True,
                                    )

                                # Add an optional top to the barrier if the height is more than 1
                                if wallHeight > 1:
                                    setBlock(
                                        stone_brick_slab,
                                        coordBresenham[0],
                                        groundLevel + wallHeight + 1,
                                        coordBresenham[1],
                                        overrideAll=True,
                                    )

    saveRegions()

    print(
        f"Processing finished in {(time() - startTime):.2f} seconds"
        + f" ({((time() - startTime) / 60):.2f} minutes)"
    )
