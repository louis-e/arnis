$execute store result score #b arnis.walk run data get storage arnis:walk seg[$(s)].b
$execute store result score @s arnis.ws run data get storage arnis:walk seg[$(s)].n[$(k)]
scoreboard players set @s arnis.wi 0
# Turned round at a dead end or an obstacle: on from beside where this lane stopped.
$execute if score @s arnis.ws = #b arnis.walk store result score @s arnis.wi run data get storage arnis:walk seg[$(s)].r[-1]
function arnis:walk/enter
