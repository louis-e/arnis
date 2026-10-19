kill @e[type=marker,tag=arnis.ob.heart]
scoreboard players set #won arnis.ob 1
scoreboard players set #boss arnis.ob 0
bossbar set arnis:boss visible false
title @a times 10 120 40
function arnis:outbreak/text/victory
title @a title {"text":"Victory","color":"gold"}
execute as @a at @s run playsound minecraft:ui.toast.challenge_complete master @s ~ ~ ~ 1 1
execute as @a at @s run function arnis:outbreak/fireworks
tellraw @a ["",{"text":"Patient Zero is dead. You held out for ","color":"gray"},{"score":{"name":"#day","objective":"arnis.ob"},"color":"gold"},{"text":" days. The zombies keep coming as long as you stay.","color":"gray"}]
