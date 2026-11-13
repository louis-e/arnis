$execute store result score #b arnis.walk run data get storage arnis:walk seg[$(s)].b
execute if score #b arnis.walk matches ..-1 run return 0
# On from beside the point just passed, along the lane back.
scoreboard players operation #j arnis.walk = @s arnis.wi
scoreboard players remove #j arnis.walk 1
execute if score #j arnis.walk matches ..-1 run scoreboard players set #j arnis.walk 0
execute store result storage arnis:walk a.j int 1 run scoreboard players get #j arnis.walk
function arnis:walk/turn_back_to with storage arnis:walk a
