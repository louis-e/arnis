scoreboard players set #won arnis.ds 1
title @a times 10 120 40
function arnis:disaster/text/victory
title @a title {"text":"Survived","color":"gold"}
execute as @a at @s run playsound minecraft:ui.toast.challenge_complete master @s ~ ~ ~ 1 1
execute in minecraft:overworld as @a[gamemode=!spectator,distance=0..] at @s run function arnis:disaster/fireworks
tellraw @a {"text":"That was all of them. They keep coming, and harder, for as long as you stay.","color":"gray"}
