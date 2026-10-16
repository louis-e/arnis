scoreboard objectives add arnis.car dummy
scoreboard players set #car_t arnis 0
scoreboard players set #2 arnis 2
scoreboard players set #4 arnis 4
scoreboard players set #8 arnis 8
scoreboard players set #32 arnis 32
scoreboard players set #45 arnis 45
scoreboard players set #1000 arnis 1000
function arnis:cars/network
execute store result score #segs arnis.car run data get storage arnis:cars seg
execute unless score #pending arnis.car matches 0.. run scoreboard players set #pending arnis.car 0
