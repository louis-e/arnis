execute if score #budget arnis.ds matches ..0 run return 0
execute store result score #px arnis.ds run data get entity @s Pos[0]
execute store result score #pz arnis.ds run data get entity @s Pos[2]
scoreboard players operation #px arnis.ds /= #64 arnis
scoreboard players operation #pz arnis.ds /= #64 arnis
function arnis:disaster/water/ring
