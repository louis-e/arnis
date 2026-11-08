execute if score @s arnis.ww matches 1.. run return run function arnis:walk/hold
execute store result score #in_way arnis.walk run function arnis:walk/ahead
execute if score #in_way arnis.walk matches 1 run return run function arnis:walk/blocked
scoreboard players set @s arnis.wb 0
scoreboard players remove @s arnis.wn 1
execute if score @s arnis.wn matches ..0 run return run function arnis:walk/arrive
function arnis:walk/move
