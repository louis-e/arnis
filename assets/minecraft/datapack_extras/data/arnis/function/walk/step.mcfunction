execute if score @s arnis.ww matches 1.. run return run function arnis:walk/hold
execute store result score #in_way arnis.walk run function arnis:walk/ahead
execute if score #in_way arnis.walk matches 1 run return run function arnis:walk/blocked
execute if score #in_way arnis.walk matches 4 run return run scoreboard players add @s arnis.wb 1
# A sidestep is the step: the move below goes from where this one began.
execute if score #in_way arnis.walk matches 3 unless entity @s[tag=arnis.w_aside] run return run function arnis:walk/aside
execute if score #in_way arnis.walk matches 2 if score #lead arnis.walk < @s arnis.wv run function arnis:walk/slow
execute if score @s arnis.wb matches ..199 run scoreboard players set @s arnis.wb 0
scoreboard players remove @s arnis.wn 1
execute if score @s arnis.wn matches ..0 run return run function arnis:walk/arrive
function arnis:walk/move
