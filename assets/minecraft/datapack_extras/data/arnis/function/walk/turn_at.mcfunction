$execute store result score #k arnis.walk run data get storage arnis:walk seg[$(s)].n
# Off the edge of the area, or nowhere at all to go: gone.
execute if score #k arnis.walk matches ..0 run return run function arnis:walk/gone
function arnis:walk/chance
scoreboard players operation #r arnis.walk %= #k arnis.walk
execute store result storage arnis:walk a.k int 1 run scoreboard players get #r arnis.walk
function arnis:walk/onto with storage arnis:walk a
