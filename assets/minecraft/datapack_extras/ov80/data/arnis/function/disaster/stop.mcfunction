function arnis:disaster/clear
scoreboard players set #state arnis.ds -1
scoreboard objectives setdisplay sidebar
# Hand the sidebar to the other mode if it still runs.
execute if score #state arnis.ob matches 1 run scoreboard objectives setdisplay sidebar arnis.ob.kills
bossbar set arnis:disaster visible false
tellraw @a {"text":"Disasters stopped. Start them again from the pause menu.","color":"gray"}
