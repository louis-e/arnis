function arnis:outbreak/clear
scoreboard players set #state arnis.ob -1
scoreboard objectives setdisplay sidebar
# Hand the sidebar to the other mode if it still runs.
execute if score #state arnis.ds matches 1 run scoreboard objectives setdisplay sidebar arnis.ds.won
bossbar set arnis:outbreak visible false
tellraw @a {"text":"Outbreak stopped. Start it again from the pause menu.","color":"gray"}
