execute store result score #n arnis.ds if entity @e[type=marker,tag=arnis.ds.front]
execute if score #n arnis.ds >= #cap arnis.ds run return 0
execute summon marker run function arnis:disaster/wildfire/place
