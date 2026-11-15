# One mast or boom takes what its crossing, number x, shows now.
scoreboard players operation @s arnis.xs = #epoch arnis.xing
scoreboard players set #n arnis.xing 0
$scoreboard players operation #n arnis.xing = #x$(x) arnis.xing
function arnis:xing/apply
