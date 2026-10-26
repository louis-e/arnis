playsound minecraft:block.deepslate.break ambient @s ~ ~-1 ~ 1 0.5
# Indoors, things come down from above.
function arnis:disaster/util/outdoors
execute if score #out arnis.ds matches 0 store result score #r arnis.ds run random value 1..5
execute if score #out arnis.ds matches 0 if score #r arnis.ds matches 1..2 run function arnis:disaster/quake/falling
execute store result score #py arnis.ds run data get entity @s Pos[1]
execute summon marker run function arnis:disaster/quake/damage
execute summon marker run function arnis:disaster/quake/damage
execute summon marker run function arnis:disaster/quake/damage
execute if score #pass arnis.ds matches 2.. summon marker run function arnis:disaster/quake/damage
