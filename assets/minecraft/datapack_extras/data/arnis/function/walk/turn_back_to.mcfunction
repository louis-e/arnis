$execute store result score #j arnis.walk run data get storage arnis:walk seg[$(s)].r[$(j)]
# No way back within reach here: keep waiting.
execute if score #j arnis.walk matches ..-1 run return 0
scoreboard players operation @s arnis.wi = #j arnis.walk
scoreboard players operation @s arnis.ws = #b arnis.walk
function arnis:walk/enter
