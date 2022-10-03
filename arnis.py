#!/usr/bin/env python

# Copyright 2022 by louis-e, https://github.com/louis-e/.
# MIT License
# Please see the LICENSE file that should have been included as part of this package.

import os, sys, time, gc, requests, cv2, argparse, anvil
from random import choice, randint
from math import floor
import numpy as np
import matplotlib.path as mplPath
from polylabel import polylabel

parser = argparse.ArgumentParser(description='Arnis - Generate cities from real life in Minecraft using Python')
parser.add_argument("--city", dest="city", help="Name of the city")
parser.add_argument("--state", dest="state", help="Name of the state")
parser.add_argument("--country", dest="country", help="Name of the country")
parser.add_argument("--path", dest="path", help="Path to the minecraft world")
parser.add_argument("--debug", dest="debug", default=False, action="store_true", help="Enable debug mode")
args = parser.parse_args()
if (args.city is None or args.state is None or args.country is None or args.path is None):
    print("Error! Missing arguments")
    os._exit(1)

gc.collect()
np.seterr(all='raise')
np.set_printoptions(threshold=sys.maxsize)

def floodFill(img, px, py, newColor, currentBuilding, elementType="None"):
    startTimeFloodfill = time.time()
    currentBuilding = np.delete(currentBuilding, 0, axis=0)
    if (len(currentBuilding) <= 2): return img
    if not (mplPath.Path(currentBuilding).contains_point((py, px))):
        centroid = polylabel([currentBuilding.tolist()], with_distance=True)
        px = round(centroid[0][1])
        py = round(centroid[0][0])
        if not (mplPath.Path(currentBuilding).contains_point((py, px))):
            if (mplPath.Path(currentBuilding).contains_point((py - 5, px))):
                py -= 5
            elif (mplPath.Path(currentBuilding).contains_point((py + 5, px))):
                py += 5
            elif (mplPath.Path(currentBuilding).contains_point((py, px - 5))):
                px -= 5
            elif (mplPath.Path(currentBuilding).contains_point((py, px + 5))):
                px += 5
            else: return img

    if (str(img[px][py][0])[:1] == "5" or str(img[px][py][0])[:1] == "6"):
        if (mplPath.Path(currentBuilding).contains_point((py - 1, px))):
            py -= 1
        elif (mplPath.Path(currentBuilding).contains_point((py + 1, px))):
            py += 1
        elif (mplPath.Path(currentBuilding).contains_point((py, px - 1))):
            px -= 1
        elif (mplPath.Path(currentBuilding).contains_point((py, px + 1))):
            px += 1
        else: return img

    try: oldColor = img[px][py][0]
    except Exception: return img
    queue = [(px, py)]
    seen = set()
    tot_rows = img.shape[0]
    tot_cols = img.shape[1]
    queueLen = 0
    while queue:
        nxt = []
        for x, y in queue:
            if (img[x][y] == newColor):
                continue
            if not (mplPath.Path(currentBuilding).contains_point((y, x))):
                return img
            img[x][y] = newColor
            seen.add((x, y))
            
            if x and (x - 1, y) not in seen and (img[x - 1][y] == oldColor or (elementType == "building" and str(img[x - 1][y][0])[:1] == "1")):
                nxt.append((x - 1, y))
            if y and (x, y - 1) not in seen and (img[x][y - 1] == oldColor or (elementType == "building" and str(img[x][y - 1][0])[:1] == "1")):
                nxt.append((x, y - 1))
            if x < tot_rows - 1 and (x + 1, y) not in seen and (img[x + 1][y] == oldColor or (elementType == "building" and str(img[x + 1][y][0])[:1] == "1")):
                nxt.append((x + 1, y))
            if y < tot_cols - 1 and (x, y + 1) not in seen and (img[x][y + 1] == oldColor or (elementType == "building" and str(img[x][y + 1][0])[:1] == "1")):
                nxt.append((x, y + 1))

            if (time.time() - startTimeFloodfill > 7 or (elementType == "tree_row" and time.time() - startTimeFloodfill > 0.3)): # Timeout (known issue, see Github readme)
                return img

        queue = nxt
        if (len(nxt) > queueLen):
            queueLen = len(nxt)

    return img

def bresenham(x1, y1, x2, y2): #Bresenham Line Algorithm Credit: encukou/bresenham@Github
    dx = x2 - x1
    dy = y2 - y1

    xsign = 1 if dx > 0 else -1
    ysign = 1 if dy > 0 else -1

    dx = abs(dx)
    dy = abs(dy)

    if dx > dy:
        xx, xy, yx, yy = xsign, 0, 0, ysign
    else:
        dx, dy = dy, dx
        xx, xy, yx, yy = 0, ysign, xsign, 0

    D = 2*dy - dx
    y = 0

    for x in range(dx + 1):
        yield x1 + x*xx + y*yx, y1 + x*xy + y*yy
        if D >= 0:
            y += 1
            D -= 2*dx
        D += 2*dy

def getData(city, state, country):
    print("Fetching data...")
    api_servers = ['https://overpass-api.de/api/interpreter', 'https://lz4.overpass-api.de/api/interpreter', 'https://z.overpass-api.de/api/interpreter', 'https://maps.mail.ru/osm/tools/overpass/api/interpreter', 'https://overpass.openstreetmap.ru/api/interpreter', 'https://overpass.kumi.systems/api/interpreter']
    url = choice(api_servers)
    query1 = f"""
        [out:json];
        area[name=""" + '"' + city.replace(" ", "-") + '"' + """]->.city;
        area[name=""" + '"' + state.replace(" ", "-") + '"' + """]->.state;
        area[name=""" + '"' + country.replace(" ", "-") + '"' + """]->.country;
        way(area.country)(area.state)(area.city)[!power][!place][!ferry];
        (._;>;);
        out;
    """

    print("Chosen server: " + url)
    try:
        data = requests.get(url, params={'data': query1}).json()

        if (len(data['elements']) == 0):
            print("Error! No data available")
            os._exit(1)
    except Exception as e:
        if "The server is probably too busy to handle your request." in str(e):
            print("Error! OSM server overloaded")
        elif "Dispatcher_Client::request_read_and_idx::rate_limited" in str(e):
            print("Error! IP rate limited")
        else:
            print("Error! " + str(e))
        os._exit(1)

    if (args.debug):
        with open('arnis-debug-raw_data.json', 'w', encoding="utf-8") as f:
            f.write(str(data))
    return data

def processData(data):
    print("Parsing data...")
    resDownScaler = 100
    processingStartTime = time.time()

    greatestElementX = 0
    greatestElementY = 0
    for element in data['elements']:
        if (element['type'] == 'node'):
            element['lat'] = int(str(element['lat']).replace('.', ''))
            element['lon'] = int(str(element['lon']).replace('.', ''))

            if (element['lat'] > greatestElementX):
                greatestElementX = element['lat']
            if (element['lon'] > greatestElementY):
                greatestElementY = element['lon']

    for element in data['elements']:
        if (element['type'] == 'node'):
            if (len(str(element['lat'])) != len(str(greatestElementX))):
                for i in range(0, len(str(greatestElementX)) - len(str(element['lat']))):
                    element['lat'] *= 10
                    
            if (len(str(element['lon'])) != len(str(greatestElementY))):
                for i in range(0, len(str(greatestElementY)) - len(str(element['lon']))):
                    element['lon'] *= 10

    lowestElementX = greatestElementX
    lowestElementY = greatestElementY
    for element in data['elements']:
        if (element['type'] == 'node'):
            if (element['lat'] < lowestElementX):
                lowestElementX = element['lat']
            if (element['lon'] < lowestElementY):
                lowestElementY = element['lon']

    nodesDict = {}
    for element in data['elements']:
        if (element['type'] == 'node'):
            element['lat'] -= lowestElementX
            element['lon'] -= lowestElementY
            nodesDict[element['id']] = [element['lat'], element['lon']]    


    img = np.zeros((round((greatestElementY - lowestElementY) / resDownScaler) + 5, round((greatestElementX - lowestElementX) / resDownScaler) + 5, 1), np.uint8)
    img.fill(0)
    imgLanduse = img.copy()
    origImgSize = img.shape[0] * img.shape[1]

    orig_posDeterminationCoordX = 0
    orig_posDeterminationCoordY = 0
    map_posDeterminationCoordX = 0
    map_posDeterminationCoordY = 0
    nodeIndexList = []
    for i, element in enumerate(data['elements']):
        if (element['type'] == 'way'):
            for j, node in enumerate(element['nodes']):
                element['nodes'][j] = nodesDict[node]
                
            if ("tags" in element and "building" in element["tags"] and orig_posDeterminationCoordX == 0):
                orig_posDeterminationCoordX = element['nodes'][0][0]
                orig_posDeterminationCoordY = element['nodes'][0][1]
                map_posDeterminationCoordX = round(element['nodes'][0][0] / resDownScaler)
                map_posDeterminationCoordY = round(element['nodes'][0][1] / resDownScaler)
        elif (element['type'] == 'node'):
            nodeIndexList.append(i)

    for i in reversed(nodeIndexList):
        del data['elements'][i]

    if (args.debug):
        print("Biggest element X: " + str(greatestElementX))
        print("Biggest element Y: " + str(greatestElementY))
        print("Lowest element X: " + str(lowestElementX))
        print("Lowest element Y: " + str(lowestElementY))
        print("Original position determination reference coordinates: " + str(orig_posDeterminationCoordX) + ", " + str(orig_posDeterminationCoordY))
        print("Map position determination reference coordinates: " + str(map_posDeterminationCoordX) + ", " + str(map_posDeterminationCoordY))
        with open('arnis-debug-processed_data.json', 'w', encoding="utf-8") as f:
            f.write(str(data))
    print("Processing data...")
    
    maxBuilding = (0, 0)
    minBuilding = (greatestElementX, greatestElementY)
    ElementIncr = 0
    ElementsLen = len(data['elements'])
    lastProgressPercentage = 0
    for element in reversed(data['elements']):
        progressPercentage = round(100 * (ElementIncr + 1) / ElementsLen)
        if (progressPercentage % 10 == 0 and progressPercentage != lastProgressPercentage):
            print("Element " + str(ElementIncr + 1) + "/" + str(ElementsLen) + " (" + str(progressPercentage) + "%)")
            lastProgressPercentage = progressPercentage

        if (element['type'] == 'way' and "tags" in element):
            if ("building" in element['tags']):
                previousElement = (0, 0)
                cornerAddup = (0, 0, 0)
                currentBuilding = np.array([[0, 0]])
                for coordinate in element['nodes']:
                    cordX = round(coordinate[0] / resDownScaler)
                    cordY = round(coordinate[1] / resDownScaler)
                    buildingHeight = 1

                    if (cordX > maxBuilding[0]):
                        maxBuilding = (cordX, maxBuilding[1])
                    if (cordY > maxBuilding[1]):
                        maxBuilding = (maxBuilding[0], cordY)

                    if (cordX < minBuilding[0]):
                        minBuilding = (cordX, minBuilding[1])
                    if (cordY < minBuilding[1]):
                        minBuilding = (minBuilding[0], cordY)

                    if (previousElement != (0, 0)):
                        if ("height" in element['tags']):
                            if (len(element['tags']['height']) >= 3):
                                buildingHeight = 9
                            elif (len(element['tags']['height']) == 1):
                                buildingHeight = 2
                            elif (element['tags']['height'][:1] == "1"):
                                buildingHeight = 3
                            elif (element['tags']['height'][:1] == "2"):
                                buildingHeight = 6
                            else:
                                buildingHeight = 9
                        
                        if ("building:levels" in element['tags'] and int(element['tags']['building:levels']) <= 8 and int(element['tags']['building:levels']) >= 1):
                            buildingHeight = str(int(element['tags']['building:levels']) - 1)

                        for i in bresenham(cordX, cordY, previousElement[0], previousElement[1]):
                            if not (str(img[i[1]][i[0]][0])[:1] == "6" and img[i[1]][i[0]][0] > int("6" + str(buildingHeight))):
                                img[i[1]][i[0]] = int("6" + str(buildingHeight))
                                
                        currentBuilding = np.append(currentBuilding, [[cordX, cordY]], axis=0)
                        if not (str(img[cordY][cordX][0])[:1] == "5" and img[cordY][cordX][0] > int("5" + str(buildingHeight))):
                            img[cordY][cordX] = int("5" + str(buildingHeight))
                            
                        if not (str(img[previousElement[1]][previousElement[0]][0])[:1] == "5" and img[previousElement[1]][previousElement[0]][0] > int("5" + str(buildingHeight))):
                            img[previousElement[1]][previousElement[0]] = int("5" + str(buildingHeight))
                            
                        cornerAddup = (cornerAddup[0] + cordX, cornerAddup[1] + cordY, cornerAddup[2] + 1)
                    previousElement = (cordX, cordY)

                if (cornerAddup != (0, 0, 0)):
                    img = floodFill(img, round(cornerAddup[1] / cornerAddup[2]), round(cornerAddup[0] / cornerAddup[2]), int("7" + str(buildingHeight)), currentBuilding, elementType="building")
                    

            elif ("highway" in element['tags']):
                previousElement = (0, 0)
                for coordinate in element['nodes']:
                    cordX = round(map_posDeterminationCoordX * coordinate[0] / orig_posDeterminationCoordX)
                    cordY = round(map_posDeterminationCoordY * coordinate[1] / orig_posDeterminationCoordY)
                    highwayType = 10
                    if (previousElement != (0, 0) and element['tags']['highway'] != "corridor" and previousElement != (0, 0) and element['tags']['highway'] != "steps" and element['tags']['highway'] != "bridge"):
                        blockRange = 2
                        highwayType = 10

                        if (element['tags']['highway'] == "path" or element['tags']['highway'] == "footway"):
                            blockRange = 1
                            highwayType = 11
                        elif (element['tags']['highway'] == "motorway"):
                            blockRange = 4
                        elif (element['tags']['highway'] == "track"):
                            blockRange = 1
                            highwayType = 12
                        elif ("lanes" in element['tags'] and element['tags']['lanes'] != "1" and element['tags']['lanes'] != "2"):
                            blockRange = 4
                        
                        for i in bresenham(cordX, cordY, previousElement[0], previousElement[1]):
                            for x in range(i[0] - blockRange, i[0] + blockRange + 1):
                                for y in range(i[1] - blockRange, i[1] + blockRange + 1):
                                    if (img[y][x] == 0): img[y][x] = highwayType
                    previousElement = (cordX, cordY)


            elif ("landuse" in element['tags']):
                previousElement = (0, 0)
                cornerAddup = (0, 0, 0)
                currentLanduse = np.array([[0, 0]])
                for coordinate in element['nodes']:
                    cordX = round(coordinate[0] / resDownScaler)
                    cordY = round(coordinate[1] / resDownScaler)
                    landuseType = 39
                    if (previousElement != (0, 0) and element['tags']['landuse'] != "industrial" and element['tags']['landuse'] != "residential"):
                        if (element['tags']['landuse'] == "greenfield" or element['tags']['landuse'] == "meadow" or element['tags']['landuse'] == "grass"):
                            landuseType = 30
                        elif (element['tags']['landuse'] == "farmland"):
                            landuseType = 31
                        elif (element['tags']['landuse'] == "forest"):
                            landuseType = 32
                        elif (element['tags']['landuse'] == "cemetery"):
                            landuseType = 33
                        elif (element['tags']['landuse'] == "beach"):
                            landuseType = 34

                        for i in bresenham(cordX, cordY, previousElement[0], previousElement[1]):
                            if (imgLanduse[i[1]][i[0]] == 0): imgLanduse[i[1]][i[0]] = landuseType
                        
                        currentLanduse = np.append(currentLanduse, [[cordX, cordY]], axis=0)
                        cornerAddup = (cornerAddup[0] + cordX, cornerAddup[1] + cordY, cornerAddup[2] + 1)
                    previousElement = (cordX, cordY)

                if (cornerAddup != (0, 0, 0)):
                    imgLanduse = floodFill(imgLanduse, round(cornerAddup[1] / cornerAddup[2]), round(cornerAddup[0] / cornerAddup[2]), landuseType, currentLanduse)
            

            elif ("natural" in element['tags']):
                previousElement = (0, 0)
                cornerAddup = (0, 0, 0)
                currentNatural = np.array([[0, 0]])
                for coordinate in element['nodes']:
                    cordX = round(coordinate[0] / resDownScaler)
                    cordY = round(coordinate[1] / resDownScaler)
                    naturalType = 39
                    if (previousElement != (0, 0)):
                        if (element['tags']['natural'] == "scrub" or element['tags']['natural'] == "grassland"):
                            naturalType = 30
                        elif (element['tags']['natural'] == "beach" or element['tags']['natural'] == "sand"):
                            naturalType = 34
                        elif (element['tags']['natural'] == "wood" or element['tags']['natural'] == "tree_row"):
                            naturalType = 32
                        elif (element['tags']['natural'] == "wetland"):
                            naturalType = 35
                        elif (element['tags']['natural'] == "water"):
                            naturalType = 38

                        for i in bresenham(cordX, cordY, previousElement[0], previousElement[1]):
                            if (imgLanduse[i[1]][i[0]] == 0): imgLanduse[i[1]][i[0]] = naturalType
                        
                        currentNatural = np.append(currentNatural, [[cordX, cordY]], axis=0)
                        cornerAddup = (cornerAddup[0] + cordX, cornerAddup[1] + cordY, cornerAddup[2] + 1)
                    previousElement = (cordX, cordY)

                if (cornerAddup != (0, 0, 0)):
                    if (naturalType != 32):
                        imgLanduse = floodFill(imgLanduse, round(cornerAddup[1] / cornerAddup[2]), round(cornerAddup[0] / cornerAddup[2]), naturalType, currentNatural)
                    else:
                        imgLanduse = floodFill(imgLanduse, round(cornerAddup[1] / cornerAddup[2]), round(cornerAddup[0] / cornerAddup[2]), naturalType, currentNatural, elementType="tree_row")


            elif ("leisure" in element['tags']):
                previousElement = (0, 0)
                cornerAddup = (0, 0, 0)
                currentLeisure = np.array([[0, 0]])
                for coordinate in element['nodes']:
                    cordX = round(coordinate[0] / resDownScaler)
                    cordY = round(coordinate[1] / resDownScaler)
                    leisureType = 39
                    if (previousElement != (0, 0) and element['tags']['leisure'] != "marina"):
                        if (element['tags']['leisure'] == "park" or element['tags']['leisure'] == "playground" or element['tags']['leisure'] == "garden"):
                            leisureType = 30
                        elif (element['tags']['leisure'] == "pitch"):
                            leisureType = 36
                        elif (element['tags']['leisure'] == "swimming_pool"):
                            leisureType = 37

                        for i in bresenham(cordX, cordY, previousElement[0], previousElement[1]):
                            if (imgLanduse[i[1]][i[0]] == 0): imgLanduse[i[1]][i[0]] = leisureType
                        
                        currentLeisure = np.append(currentLeisure, [[cordX, cordY]], axis=0)
                        cornerAddup = (cornerAddup[0] + cordX, cornerAddup[1] + cordY, cornerAddup[2] + 1)
                    previousElement = (cordX, cordY)
                    
                if (cornerAddup != (0, 0, 0)):
                    imgLanduse = floodFill(imgLanduse, round(cornerAddup[1] / cornerAddup[2]), round(cornerAddup[0] / cornerAddup[2]), leisureType, currentLeisure)
            
                
            elif ("waterway" in element['tags']):
                    previousElement = (0, 0)
                    for coordinate in element['nodes']:
                        cordX = round(coordinate[0] / resDownScaler)
                        cordY = round(coordinate[1] / resDownScaler)

                        if (previousElement != (0, 0) and not ("layer" in element['tags'] and (element['tags']['layer'] == "-1" or element['tags']['layer'] == "-2" or element['tags']['layer'] != "-3"))):
                            waterwayWidth = 4
                            if ("width" in element['tags']):
                                try: waterwayWidth = int(element['tags']['width'])
                                except Exception as e: waterwayWidth = int(float(element['tags']['width']))
                            
                            for i in bresenham(cordX, cordY, previousElement[0], previousElement[1]):
                                for x in range(round(i[0] - waterwayWidth / 2), round(i[0] + waterwayWidth + 1 / 2)):
                                    for y in range(round(i[1] - waterwayWidth / 2), round(i[1] + waterwayWidth + 1 / 2)):
                                        if (img[y][x] != 13): img[y][x] = 38
                        previousElement = (cordX, cordY)


            elif ("amenity" in element['tags']):
                previousElement = (0, 0)
                cornerAddup = (0, 0, 0)
                currentAmenity = np.array([[0, 0]])
                amenityType = 20
                for coordinate in element['nodes']:
                    cordX = round(coordinate[0] / resDownScaler)
                    cordY = round(coordinate[1] / resDownScaler)
                    if (previousElement != (0, 0) and (element['tags']['amenity'] == "parking" or element['tags']['amenity'] == "fountain")):
                        if (element['tags']['amenity'] == "parking"):
                            amenityType = 20
                        elif (element['tags']['amenity'] == "fountain"):
                            amenityType = 21

                        for i in bresenham(cordX, cordY, previousElement[0], previousElement[1]):
                            if (imgLanduse[i[1]][i[0]] == 0): imgLanduse[i[1]][i[0]] = amenityType

                        currentAmenity = np.append(currentAmenity, [[cordX, cordY]], axis=0)
                        cornerAddup = (cornerAddup[0] + cordX, cornerAddup[1] + cordY, cornerAddup[2] + 1)
                    previousElement = (cordX, cordY)

                if (amenityType == 21): amenityType = 37
                if (cornerAddup != (0, 0, 0)):
                    imgLanduse = floodFill(imgLanduse, round(cornerAddup[1] / cornerAddup[2]), round(cornerAddup[0] / cornerAddup[2]), amenityType, currentAmenity)
            
                
            elif ("bridge" in element['tags']):
                    previousElement = (0, 0)
                    for coordinate in element['nodes']:
                        cordX = round(coordinate[0] / resDownScaler)
                        cordY = round(coordinate[1] / resDownScaler)

                        if (previousElement != (0, 0)):
                            for i in bresenham(cordX, cordY, previousElement[0], previousElement[1]):
                                img[i[1]][i[0]] = 13
                        previousElement = (cordX, cordY)

                        
            elif ("railway" in element['tags']):
                    previousElement = (0, 0)
                    for coordinate in element['nodes']:
                        cordX = round(coordinate[0] / resDownScaler)
                        cordY = round(coordinate[1] / resDownScaler)

                        if (previousElement != (0, 0) and element['tags']['railway'] != "proposed"):
                            for i in bresenham(cordX - 2, cordY - 2, previousElement[0] - 2, previousElement[1] - 2):
                                img[i[1]][i[0]] = 14
                            for i in bresenham(cordX + 1, cordY + 1, previousElement[0] + 1, previousElement[1] + 1):
                                img[i[1]][i[0]] = 14
                        previousElement = (cordX, cordY)
                        
            elif ("barrier" in element['tags']):
                    previousElement = (0, 0)
                    for coordinate in element['nodes']:
                        cordX = round(coordinate[0] / resDownScaler)
                        cordY = round(coordinate[1] / resDownScaler)

                        if (previousElement != (0, 0)):
                            wallHeight = 1
                            if ("height" in element['tags']):
                                wallHeight = round(int(float(element['tags']['height'])))
                            if (wallHeight > 3): wallHeight = 2

                            for i in bresenham(cordX, cordY, previousElement[0], previousElement[1]):
                                if (str(img[i[1]][i[0]][0])[:1] != 5 and str(img[i[1]][i[0]][0])[:1] != 6 and str(img[i[1]][i[0]][0])[:1] != 7): img[i[1]][i[0]] = int("2" + str((wallHeight + 1)))
                        previousElement = (cordX, cordY)
            
            ElementIncr += 1


    print("Optimizing data...")
    
    minBuilding = (minBuilding[0] - 50, minBuilding[1] - 50)
    maxBuilding = (maxBuilding[0] + 50, maxBuilding[1] + 50)
    img = img[minBuilding[1]:maxBuilding[1], minBuilding[0]:maxBuilding[0]]
    imgLanduse = imgLanduse[minBuilding[1]:maxBuilding[1], minBuilding[0]:maxBuilding[0]]
    print(str(100 - round(100 * (img.shape[0] * img.shape[1]) / origImgSize)) + "% size reduction")
    for x in range(0, img.shape[0]):
        for y in range(0, img.shape[1]):
            if (imgLanduse[x][y] != 0 and img[x][y] == 0):
                img[x][y] = imgLanduse[x][y]

    print("Processing finished in " + str(round(time.time() - processingStartTime, 2)) + " seconds (" + str(round((time.time() - processingStartTime) / 60, 2)) + " minutes)")
    if (args.debug): cv2.imwrite('arnis-debug-map.png', img)
    return np.flip(img, axis=1)


processStartTime = time.time()
air = anvil.Block('minecraft', 'air')
stone = anvil.Block('minecraft', 'stone')
grass_block = anvil.Block('minecraft', 'grass_block')
dirt = anvil.Block('minecraft', 'dirt')
sand = anvil.Block('minecraft', 'sand')
podzol = anvil.Block.from_numeric_id(3, 2)
grass = anvil.Block.from_numeric_id(175, 2)
farmland = anvil.Block('minecraft', 'farmland')
water = anvil.Block('minecraft', 'water')
wheat = anvil.Block('minecraft', 'wheat')
carrots = anvil.Block('minecraft', 'carrots')
potatoes = anvil.Block('minecraft', 'potatoes')
cobblestone = anvil.Block('minecraft', 'cobblestone')
iron_block = anvil.Block('minecraft', 'iron_block')
log = anvil.Block.from_numeric_id(17)
leaves = anvil.Block.from_numeric_id(18)
white_stained_glass = anvil.Block('minecraft', 'white_stained_glass')
dark_oak_door_lower = anvil.Block('minecraft', 'dark_oak_door', properties={'half': 'lower'})
dark_oak_door_upper = anvil.Block('minecraft', 'dark_oak_door', properties={'half': 'upper'})
cobblestone_wall = anvil.Block('minecraft', 'cobblestone_wall')
stone_brick_slab = anvil.Block.from_numeric_id(44, 5)
red_flower = anvil.Block.from_numeric_id(38)
white_concrete = anvil.Block('minecraft', 'white_concrete')
black_concrete = anvil.Block('minecraft', 'black_concrete')
gray_concrete = anvil.Block('minecraft', 'gray_concrete')
light_gray_concrete = anvil.Block('minecraft', 'light_gray_concrete')
green_stained_hardened_clay = anvil.Block.from_numeric_id(159, 5)
dirt = anvil.Block('minecraft', 'dirt')
glowstone = anvil.Block('minecraft', 'glowstone')
sponge = anvil.Block('minecraft', 'sponge')

regions = {}
for x in range(0, 3):
    for z in range(0, 3):
        regions['r.' + str(x) + '.' + str(z)] = anvil.EmptyRegion(0, 0)


def setBlock(block, x, y, z):
    flooredX = floor(x / 512)
    flooredZ = floor(z / 512)
    identifier = 'r.' + str(flooredX) + '.' + str(flooredZ)
    if (identifier not in regions):
        regions[identifier] = anvil.EmptyRegion(0, 0)
    regions[identifier].set_block(block, x - flooredX * 512, y, z - flooredZ * 512)

def fillBlocks(block, x1, y1, z1, x2, y2, z2):
    for x in range(x1, x2 + 1):
        for y in range(y1, y2 + 1):
            for z in range(z1, z2 + 1):
                if not (x == x2 + 1 or y == y2 + 1 or z == z2 + 1):
                    setBlock(block, x, y, z)

mcWorldPath = args.path
if (mcWorldPath[-1] == '/'): mcWorldPath = mcWorldPath[:-1]
def saveRegion(region='all'):
    if (region == 'all'):
        for key in regions:
            regions[key].save(mcWorldPath + '/region/' + key + '.mca')
            print("Saved " + key)
    else:
        regions[region].save(mcWorldPath + '/region/' + region + '.mca')
        print("Saved " + region)

rawdata = getData(args.city, args.state, args.country)
imgarray = processData(rawdata)


print("Generating minecraft world...")

x = 0
z = 0
doorIncrement = 0
ElementIncr = 0
ElementsLen = len(imgarray)
lastProgressPercentage = 0
for i in imgarray:
    progressPercentage = round(100 * (ElementIncr + 1) / ElementsLen)
    if (progressPercentage % 10 == 0 and progressPercentage != lastProgressPercentage):
        print("Pixel " + str(ElementIncr + 1) + "/" + str(ElementsLen) + " (" + str(progressPercentage) + "%)")
        lastProgressPercentage = progressPercentage

    z = 0
    for j in i:
        setBlock(dirt, x, 0, z)
        if (j == 0): # Ground
            setBlock(light_gray_concrete, x, 1, z)
        elif (j == 10): # Street
            setBlock(black_concrete, x, 1, z)
            setBlock(air, x, 2, z)
        elif (j == 11): # Footway
            setBlock(gray_concrete, x, 1, z)
            setBlock(air, x, 2, z)
        elif (j == 12): # Natural path
            setBlock(cobblestone, x, 1, z)
        elif (j == 13): # Bridge
            setBlock(light_gray_concrete, x, 2, z)
            setBlock(light_gray_concrete, x - 1, 2, z - 1)
            setBlock(light_gray_concrete, x + 1, 2, z - 1)
            setBlock(light_gray_concrete, x + 1, 2, z + 1)
            setBlock(light_gray_concrete, x - 1, 2, z + 1)
        elif (j == 14): # Railway
            setBlock(iron_block, x, 2, z)
        elif (j == 20): # Parking
            setBlock(gray_concrete, x, 1, z)
        elif (j == 21): # Fountain border
            setBlock(light_gray_concrete, x, 2, z)
            setBlock(white_concrete, x, 1, z)
        elif (j >= 22 and j <= 24): # Fence
            if (str(j)[-1] == "2"):
                setBlock(cobblestone_wall, x, 2, z)
            else:
                fillBlocks(cobblestone , x, 2, z, x, int(str(j[0])[-1]), z)

            setBlock(grass_block, x, 1, z)
        elif (j == 30): # Meadow
            setBlock(grass_block, x, 1, z)
            randomChoice = randint(0, 2)
            if (randomChoice == 0 or randomChoice == 1):
                setBlock(grass, x, 2, z)
        elif (j == 31): # Farmland
            randomChoice = randint(0, 16)
            if (randomChoice == 0):
                setBlock(water, x, 1, z)
            else:
                setBlock(farmland, x, 1, z)
                randomChoice = randint(0, 2)
                if (randomChoice == 0):
                    setBlock(wheat, x, 2, z)
                elif (randomChoice == 1):
                    setBlock(carrots, x, 2, z)
                else:
                    setBlock(potatoes, x, 2, z)
        elif (j == 32): # Forest
            setBlock(grass_block, x, 1, z)
            randomChoice = randint(0, 8)
            if (randomChoice >= 0 and randomChoice <= 5):
                setBlock(grass, x, 2, z)
            elif (randomChoice == 6):
                fillBlocks(log, x, 2, z, x, 8, z)
                fillBlocks(leaves, x - 2, 5, z - 2, x + 2, 6, z + 2)
                setBlock(air, x - 2, 6, z - 2)
                setBlock(air, x - 2, 6, z + 2)
                setBlock(air, x + 2, 6, z - 2)
                setBlock(air, x + 2, 6, z + 2)
                fillBlocks(leaves, x - 1, 7, z - 1, x + 1, 8, z + 1)
                setBlock(air, x - 1, 8, z - 1)
                setBlock(air, x - 1, 8, z + 1)
                setBlock(air, x + 1, 8, z - 1)
                setBlock(air, x + 1, 8, z + 1)
        elif (j == 33): # Cemetery
            setBlock(podzol, x, 1, z)
            randomChoice = randint(0, 100)
            if (randomChoice == 0):
                setBlock(cobblestone, x - 1, 2, z)
                setBlock(stone_brick_slab, x - 1, 3, z)
                setBlock(stone_brick_slab, x, 2, z)
                setBlock(stone_brick_slab, x + 1, 2, z)
            elif (randomChoice == 1):
                setBlock(cobblestone, x, 2, z - 1)
                setBlock(stone_brick_slab, x, 3, z - 1)
                setBlock(stone_brick_slab, x, 2, z)
                setBlock(stone_brick_slab, x, 2, z + 1)
            elif (randomChoice == 2 or randomChoice == 3):
                setBlock(red_flower, x, 2, z)
        elif (j == 34): # Beach
            setBlock(sand, x, 1, z)
        elif (j == 35): # Wetland
            randomChoice = randint(0, 2)
            if (randomChoice == 0):
                setBlock(grass_block, x, 1, z)
            else:
                setBlock(water, x, 1, z)
        elif (j == 36): # Pitch
            setBlock(green_stained_hardened_clay, x, 1, z)
        elif (j == 37): # Swimming pool
            setBlock(water, x, 1, z)
            setBlock(white_concrete, x, 0, z)
        elif (j == 38): # Water
            setBlock(water, x, 1, z)
        elif (j == 39): # Raw grass
            setBlock(grass_block, x, 1, z)
        elif (j >= 50 and j <= 59): # House corner
            building_height = 5
            if (j == 51): building_height = 8
            elif (j == 52): building_height = 11
            elif (j == 53): building_height = 14
            elif (j == 54): building_height = 17
            elif (j == 55): building_height = 20
            elif (j == 56): building_height = 23
            elif (j == 57): building_height = 26
            elif (j == 58): building_height = 29
            elif (j == 59): building_height = 32

            fillBlocks(white_concrete, x, 1, z, x, building_height, z)
        elif (j >= 60 and j <= 69): # House wall
            building_height = 4
            if (j == 61): building_height = 7
            elif (j == 62): building_height = 10
            elif (j == 63): building_height = 13
            elif (j == 64): building_height = 16
            elif (j == 65): building_height = 19
            elif (j == 66): building_height = 22
            elif (j == 67): building_height = 25
            elif (j == 68): building_height = 28
            elif (j == 69): building_height = 31

            if (doorIncrement == 25):
                fillBlocks(white_stained_glass, x, 4, z, x, building_height, z)
                setBlock(white_concrete, x, 1, z)
                setBlock(dark_oak_door_lower, x, 2, z)
                setBlock(dark_oak_door_upper, x, 3, z)
                doorIncrement = 0
            else:
                fillBlocks(white_concrete, x, 1, z, x, 2, z)
                fillBlocks(white_stained_glass, x, 3, z, x, building_height, z)
            doorIncrement += 1
            setBlock(white_concrete, x, building_height + 1, z)
        elif (j >= 70 and j <= 79): # House interior
            if (j >= 70):
                setBlock(white_concrete, x, 5, z)
                if (j >= 71):
                    setBlock(white_concrete, x, 8, z)
                    if (j >= 72):
                        setBlock(white_concrete, x, 11, z)
                        if (j >= 73):
                            setBlock(white_concrete, x, 14, z)
                            if (j >= 74):
                                setBlock(white_concrete, x, 17, z)
                                if (j >= 75):
                                    setBlock(white_concrete, x, 20, z)
                                    if (j >= 76):
                                        setBlock(white_concrete, x, 23, z)
                                        if (j >= 77):
                                            setBlock(white_concrete, x, 26, z)
                                            if (j >= 78):
                                                setBlock(white_concrete, x, 29, z)
                                                if (j >= 78):
                                                    setBlock(white_concrete, x, 32, z)

            setBlock(glowstone, x, 1, z)

        z += 1
    x += 1
    ElementIncr += 1


print("Saving minecraft world...")
saveRegion()
print("Done! Finished in " + str(round(time.time() - processStartTime, 2)) + " seconds (" + str(round((time.time() - processStartTime) / 60, 2)) + " minutes)")
os._exit(0)
