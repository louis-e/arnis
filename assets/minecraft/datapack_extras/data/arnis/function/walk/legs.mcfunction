# Steps to the point at this walker's pace, from how far off it is in thousandths of a block.
execute store result score #dx arnis.walk run data get storage arnis:walk p.x 1000
execute store result score #dy arnis.walk run data get storage arnis:walk p.y 1000
execute store result score #dz arnis.walk run data get storage arnis:walk p.z 1000
execute store result score #t arnis.walk run data get entity @s Pos[0] 1000
scoreboard players operation #dx arnis.walk -= #t arnis.walk
execute store result score #t arnis.walk run data get entity @s Pos[1] 1000
scoreboard players operation #dy arnis.walk -= #t arnis.walk
execute store result score #t arnis.walk run data get entity @s Pos[2] 1000
scoreboard players operation #dz arnis.walk -= #t arnis.walk
execute if score #dx arnis.walk matches ..-1 run scoreboard players operation #dx arnis.walk *= #-1 arnis
execute if score #dy arnis.walk matches ..-1 run scoreboard players operation #dy arnis.walk *= #-1 arnis
execute if score #dz arnis.walk matches ..-1 run scoreboard players operation #dz arnis.walk *= #-1 arnis
# The longer side and 41 % of the shorter, within 4 % of the straight line.
function arnis:walk/octagon
scoreboard players operation #dx arnis.walk = #d arnis.walk
scoreboard players operation #dz arnis.walk = #dy arnis.walk
function arnis:walk/octagon
scoreboard players operation #half arnis.walk = @s arnis.wv
scoreboard players operation #half arnis.walk /= #2 arnis
scoreboard players operation #d arnis.walk += #half arnis.walk
scoreboard players operation #d arnis.walk /= @s arnis.wv
execute if score #d arnis.walk matches ..0 run scoreboard players set #d arnis.walk 1
scoreboard players operation @s arnis.wn = #d arnis.walk
