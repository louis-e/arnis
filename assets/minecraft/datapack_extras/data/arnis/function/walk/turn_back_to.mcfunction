$execute store result score @s arnis.wi run data get storage arnis:walk seg[$(s)].r[$(j)]
scoreboard players operation @s arnis.ws = #b arnis.walk
function arnis:walk/enter
